//! External-crate compile checks for component-author API reachability.

use edgecommons::commands::{CommandInbox, CommandScope, command_handler};
use edgecommons::config::model::Config;
use edgecommons::facades::{AppCorrelation, AppFacade, PreparedAppMessage};
use edgecommons::heartbeat::InstanceConnectivity;
use edgecommons::messaging::{Message, MessagingService};
use serde_json::json;

fn prepare_through_public_facade(
    facade: &AppFacade,
    request: &Message,
) -> edgecommons::Result<PreparedAppMessage> {
    facade.prepare_correlated(
        "ImageCaptured",
        "camera/captured",
        json!({ "captureId": "cap-1" }),
        AppCorrelation::from(request),
    )
}

fn prelude_types_are_nameable(
    _prepared: Option<edgecommons::prelude::PreparedAppMessage>,
    _correlation: Option<edgecommons::prelude::AppCorrelation>,
    _outcome: Option<edgecommons::prelude::CommandOutcome>,
    _token: Option<edgecommons::prelude::DeferredReplyToken>,
    _scope: Option<edgecommons::prelude::CommandScope>,
) {
}

/// A component test must be able to construct and drive a REAL inbox from outside the crate —
/// the same reach the Java/Python/TypeScript inboxes give — and to assert the exact per-instance
/// wire element the `state` keepalive and the `status` verb both publish.
fn a_component_can_build_and_drive_a_real_inbox(
    messaging: std::sync::Arc<dyn MessagingService>,
) -> edgecommons::Result<serde_json::Value> {
    let config = std::sync::Arc::new(Config::from_value("my-adapter", "gw-01", json!({}))?);
    let inbox = CommandInbox::new(
        messaging,
        config,
        std::sync::Arc::new(|| 0),
        std::sync::Arc::new(|| Box::pin(async { true })),
        std::sync::Arc::new(|| Some(json!({}))),
        std::sync::Arc::new(Vec::new),
    );
    inbox.register(
        "sb/read",
        CommandScope::Instance,
        command_handler(|_request, addressed_instance| async move {
            Ok(Some(json!({ "instance": addressed_instance })))
        }),
    )?;
    Ok(InstanceConnectivity::of("press12", true).to_json())
}

#[test]
fn new_component_author_types_are_publicly_reachable() {
    let _prepare: fn(&AppFacade, &Message) -> edgecommons::Result<PreparedAppMessage> =
        prepare_through_public_facade;
    prelude_types_are_nameable(None, None, None, None, None);

    let _drive: fn(std::sync::Arc<dyn MessagingService>) -> edgecommons::Result<serde_json::Value> =
        a_component_can_build_and_drive_a_real_inbox;
    assert_eq!(
        InstanceConnectivity::of("press12", true).to_json(),
        json!({ "instance": "press12", "connected": true })
    );
}
