//! End-to-end tests for the standalone MQTT provider that run **entirely in-process**.
//!
//! [`FakeBroker`] is a minimal MQTT v3.1.1 server on an ephemeral loopback port. Unlike the
//! connect-only broker in `lib_inproc.rs`, it speaks the full acknowledgement surface the
//! provider depends on — SUBACK (granting *or rejecting* a filter), PUBACK for QoS 1, UNSUBACK
//! — and it loops every PUBLISH it receives back to the client. That lets these tests drive the
//! provider's real `rumqttc` event loop and assert the contracts a mocked provider cannot:
//! that a confirmed publish waits for the broker's PUBACK, that a broker's subscription refusal
//! surfaces as an error rather than a silently dead subscription, and that inbound messages are
//! routed only to the subscriptions whose filter matches.
//!
//! No broker binary, no network, no feature gates — this runs on a stock CI machine.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use edgecommons::messaging::config::MessagingConfig;
use edgecommons::messaging::provider::mqtt::MqttProvider;
use edgecommons::messaging::{Destination, MessagingProvider, Qos};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Test-wide deadline for anything that should resolve promptly.
const SOON: Duration = Duration::from_secs(5);

/// `Result::expect_err` without the `T: Debug` bound (`Subscription` and `MqttProvider` own
/// live tasks and are deliberately not `Debug`).
fn expect_err<T, E>(result: std::result::Result<T, E>, expectation: &str) -> E {
    match result {
        Ok(_) => panic!("expected an error: {expectation}"),
        Err(error) => error,
    }
}

/// Knobs the tests use to make the broker misbehave in specific, realistic ways.
#[derive(Default)]
struct BrokerBehavior {
    /// Answer every SUBSCRIBE with the 0x80 failure return code.
    refuse_subscriptions: AtomicBool,
    /// Accept QoS 1 PUBLISHes but never acknowledge them (a stalled/blackholing broker).
    withhold_pubacks: AtomicBool,
}

struct FakeBroker {
    port: u16,
    behavior: Arc<BrokerBehavior>,
}

/// Start the in-process broker and return its ephemeral port plus its behavior knobs.
async fn fake_broker() -> FakeBroker {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let behavior = Arc::new(BrokerBehavior::default());
    let conn_behavior = behavior.clone();
    tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            tokio::spawn(serve(sock, conn_behavior.clone()));
        }
    });
    FakeBroker { port, behavior }
}

/// A listener that accepts the TCP connection and then says nothing at all — the provider must
/// not treat a silent peer as a live broker.
async fn silent_listener() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((sock, _)) = listener.accept().await {
            held.push(sock); // keep the socket open, never write a CONNACK
        }
    });
    port
}

/// Read one MQTT packet: the fixed-header byte, the remaining-length varint, then the body.
async fn read_packet(sock: &mut TcpStream) -> Option<(u8, Vec<u8>)> {
    let mut header = [0u8; 1];
    sock.read_exact(&mut header).await.ok()?;
    let (mut remaining, mut multiplier) = (0usize, 1usize);
    loop {
        let mut byte = [0u8; 1];
        sock.read_exact(&mut byte).await.ok()?;
        remaining += (byte[0] & 0x7f) as usize * multiplier;
        if byte[0] & 0x80 == 0 {
            break;
        }
        multiplier *= 128;
    }
    let mut body = vec![0u8; remaining];
    if remaining > 0 {
        sock.read_exact(&mut body).await.ok()?;
    }
    Some((header[0], body))
}

/// Encode a remaining-length varint.
fn remaining_length(mut value: usize) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut byte = (value % 128) as u8;
        value /= 128;
        if value > 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            return out;
        }
    }
}

/// Serve one client connection.
async fn serve(mut sock: TcpStream, behavior: Arc<BrokerBehavior>) {
    while let Some((header, body)) = read_packet(&mut sock).await {
        let ok = match header >> 4 {
            // CONNECT -> CONNACK (accepted).
            1 => sock.write_all(&[0x20, 0x02, 0x00, 0x00]).await.is_ok(),
            // PUBLISH: ack it at QoS 1, then loop it back to the client.
            3 => handle_publish(&mut sock, header, &body, &behavior).await,
            // SUBSCRIBE -> SUBACK, one return code per requested filter.
            8 => {
                let code = if behavior.refuse_subscriptions.load(Ordering::SeqCst) {
                    0x80 // failure
                } else {
                    0x01 // granted QoS 1
                };
                let mut suback = vec![0x90, 0x03, body[0], body[1], code];
                suback[1] = 3;
                sock.write_all(&suback).await.is_ok()
            }
            // UNSUBSCRIBE -> UNSUBACK.
            10 => sock
                .write_all(&[0xB0, 0x02, body[0], body[1]])
                .await
                .is_ok(),
            // PINGREQ -> PINGRESP.
            12 => sock.write_all(&[0xD0, 0x00]).await.is_ok(),
            14 => return, // DISCONNECT
            _ => true,
        };
        if !ok {
            return;
        }
    }
}

/// PUBACK a QoS 1 publish (unless the test withheld acks) and echo the message back so that a
/// matching subscription on the same connection receives it.
async fn handle_publish(
    sock: &mut TcpStream,
    header: u8,
    body: &[u8],
    behavior: &BrokerBehavior,
) -> bool {
    let qos = (header & 0x06) >> 1;
    let topic_len = usize::from(u16::from_be_bytes([body[0], body[1]]));
    let topic = &body[2..2 + topic_len];
    let mut cursor = 2 + topic_len;
    let packet_id = if qos > 0 {
        let id = [body[cursor], body[cursor + 1]];
        cursor += 2;
        Some(id)
    } else {
        None
    };
    let payload = &body[cursor..];

    if let Some(id) = packet_id {
        if behavior.withhold_pubacks.load(Ordering::SeqCst) {
            return true; // accepted on the wire, never acknowledged
        }
        if sock.write_all(&[0x40, 0x02, id[0], id[1]]).await.is_err() {
            return false;
        }
    }

    // Loop back at QoS 0 (no packet id, nothing for the client to ack).
    let mut echo = Vec::new();
    echo.push(0x30);
    let mut variable = Vec::new();
    variable.extend_from_slice(&(topic_len as u16).to_be_bytes());
    variable.extend_from_slice(topic);
    variable.extend_from_slice(payload);
    echo.extend_from_slice(&remaining_length(variable.len()));
    echo.extend_from_slice(&variable);
    sock.write_all(&echo).await.is_ok()
}

/// A messaging config pointing the local broker at `port`, with no northbound broker.
fn local_only_config(port: u16) -> MessagingConfig {
    serde_json::from_value(serde_json::json!({
        "messaging": {
            "local": {
                "host": "127.0.0.1",
                "port": port,
                "clientId": format!("inproc-{}", uuid::Uuid::new_v4()),
            }
        }
    }))
    .expect("a valid messaging config")
}

async fn connected_provider(port: u16) -> MqttProvider {
    MqttProvider::connect(&local_only_config(port))
        .await
        .expect("the fake broker CONNACKs")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_confirmed_publish_resolves_only_on_the_brokers_puback() {
    let broker = fake_broker().await;
    let provider = connected_provider(broker.port).await;
    assert!(provider.connected(), "CONNACK marks the connection live");

    provider
        .publish_confirmed(
            "ecv1/gw/comp/main/data/t",
            b"confirmed".to_vec(),
            Destination::Local,
            Qos::AtLeastOnce,
            SOON,
        )
        .await
        .expect("the broker PUBACKs, so the publish is confirmed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_confirmed_publish_times_out_as_ambiguous_when_the_broker_never_acks() {
    let broker = fake_broker().await;
    broker
        .behavior
        .withhold_pubacks
        .store(true, Ordering::SeqCst);
    let provider = connected_provider(broker.port).await;

    let error = provider
        .publish_confirmed(
            "ecv1/gw/comp/main/data/t",
            b"never-acked".to_vec(),
            Destination::Local,
            Qos::AtLeastOnce,
            Duration::from_millis(400),
        )
        .await
        .expect_err("an unacknowledged publish must not report success");
    let text = error.to_string();
    assert!(
        text.contains("timed out") && text.contains("ambiguous"),
        "the caller must be told the outcome is unknown, not that it failed: {text}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn confirmed_publication_is_defined_only_at_qos1_with_a_positive_deadline() {
    let broker = fake_broker().await;
    let provider = connected_provider(broker.port).await;

    for qos in [Qos::AtMostOnce, Qos::ExactlyOnce] {
        let error = provider
            .publish_confirmed("t", b"x".to_vec(), Destination::Local, qos, SOON)
            .await
            .expect_err("only QoS 1 yields a PUBACK to wait for");
        assert!(error.to_string().contains("requires QoS 1"), "{error}");
    }

    let error = provider
        .publish_confirmed(
            "t",
            b"x".to_vec(),
            Destination::Local,
            Qos::AtLeastOnce,
            Duration::ZERO,
        )
        .await
        .expect_err("a zero deadline can never be satisfied");
    assert!(error.to_string().contains("positive timeout"), "{error}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_broker_that_refuses_a_subscription_surfaces_as_an_error() {
    let broker = fake_broker().await;
    broker
        .behavior
        .refuse_subscriptions
        .store(true, Ordering::SeqCst);
    let provider = connected_provider(broker.port).await;

    let error = expect_err(
        provider
            .subscribe(
                "ecv1/gw/comp/main/data/#",
                Destination::Local,
                Qos::AtLeastOnce,
                8,
            )
            .await,
        "a rejected SUBACK must not yield a silently dead subscription",
    );
    assert!(
        error.to_string().contains("rejected subscription"),
        "{error}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subscribe_delivers_only_topics_matching_the_filter_and_stops_on_unsubscribe() {
    let broker = fake_broker().await;
    let provider = connected_provider(broker.port).await;

    let mut wildcard = provider
        .subscribe(
            "ecv1/gw/comp/main/data/#",
            Destination::Local,
            Qos::AtMostOnce,
            8,
        )
        .await
        .expect("granted");

    // A topic outside the filter must not be routed to this subscription...
    provider
        .publish(
            "ecv1/gw/other/main/evt/alarm",
            b"not-mine".to_vec(),
            Destination::Local,
            Qos::AtMostOnce,
        )
        .await
        .unwrap();
    // ...while a matching one must be, with its payload byte-exact.
    provider
        .publish(
            "ecv1/gw/comp/main/data/temp",
            b"\x00\x01\xfe\xff".to_vec(),
            Destination::Local,
            Qos::AtMostOnce,
        )
        .await
        .unwrap();

    let (topic, payload) = tokio::time::timeout(SOON, wildcard.recv())
        .await
        .expect("a matching message arrives")
        .expect("the subscription is live");
    assert_eq!(topic, "ecv1/gw/comp/main/data/temp");
    assert_eq!(
        payload, b"\x00\x01\xfe\xff",
        "the payload bytes survive the round trip verbatim"
    );

    provider
        .unsubscribe("ecv1/gw/comp/main/data/#", Destination::Local)
        .await
        .expect("unsubscribed");
    provider
        .publish(
            "ecv1/gw/comp/main/data/temp",
            b"after-unsubscribe".to_vec(),
            Destination::Local,
            Qos::AtMostOnce,
        )
        .await
        .unwrap();
    let after = tokio::time::timeout(SOON, wildcard.recv())
        .await
        .expect("dropping the routing entry resolves the receiver at once");
    assert!(
        after.is_none(),
        "unsubscribe must drop the routing entry and close the delivery channel, \
         not keep feeding a filter the component no longer wants"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_acknowledged_subscribe_requires_a_positive_timeout() {
    let broker = fake_broker().await;
    let provider = connected_provider(broker.port).await;

    let error = expect_err(
        provider
            .subscribe_acknowledged("t", Destination::Local, Qos::AtMostOnce, 4, Duration::ZERO)
            .await,
        "a zero deadline cannot admit a SUBACK",
    );
    assert!(error.to_string().contains("positive timeout"), "{error}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_northbound_destination_is_rejected_when_no_northbound_broker_is_configured() {
    let broker = fake_broker().await;
    let provider = connected_provider(broker.port).await;

    // Every destination-taking entry point must refuse, not silently fall back to local.
    let error = provider
        .publish("t", b"x".to_vec(), Destination::Northbound, Qos::AtMostOnce)
        .await
        .expect_err("no northbound broker is configured");
    assert!(error.to_string().contains("not configured"), "{error}");

    assert!(
        provider
            .subscribe("t", Destination::Northbound, Qos::AtMostOnce, 4)
            .await
            .is_err(),
        "subscribe must not silently fall back to the local broker"
    );
    assert!(
        provider
            .unsubscribe("t", Destination::Northbound)
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connect_fails_when_the_peer_accepts_tcp_but_never_connacks() {
    // "Connections block until confirmed": a bare TCP accept is not a live MQTT session.
    let port = silent_listener().await;
    let error = expect_err(
        MqttProvider::connect(&local_only_config(port)).await,
        "no CONNACK means no connection",
    );
    assert!(
        error.to_string().contains("CONNACK"),
        "connect must fail on the missing CONNACK: {error}"
    );
}
