/**
 * Unit tests for the CONFIG_COMPONENT on-device provider harness (`src/config_provider.ts`).
 *
 * The module is a runnable process-entry (`void main()` at import time): it connects an
 * IPC messaging provider, subscribes to the GetConfiguration topic, replies with the
 * served config document, and installs SIGTERM/SIGINT shutdown handlers that unsubscribe +
 * disconnect before exiting. We mock the IPC provider + messaging service so no real
 * Greengrass IPC is touched, drive the subscribe handler, and exercise the shutdown path
 * (including its idempotence and disconnect ordering).
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import type { Message } from "../src/message";

// --- mocks for the IPC + messaging seam ------------------------------------

const connectMock = vi.fn();
vi.mock("../src/messaging/ipc-provider", () => ({
  IpcMessagingProvider: { connect: (...args: unknown[]) => connectMock(...args) },
}));

// Captured handler + lifecycle spies for the DefaultMessagingService instance.
let subscribeHandler: ((topic: string, request: Message) => void) | undefined;
const subscribeMock = vi.fn(async (_topic: string, handler: (t: string, r: Message) => void) => {
  subscribeHandler = handler;
});
const replyMock = vi.fn(async () => undefined);
const unsubscribeMock = vi.fn(async () => undefined);
const disconnectMock = vi.fn(async () => undefined);

vi.mock("../src/messaging/service", () => ({
  DefaultMessagingService: class {
    subscribe = subscribeMock;
    reply = replyMock;
    unsubscribe = unsubscribeMock;
    disconnect = disconnectMock;
  },
}));

const origArgv = process.argv;
const origThing = process.env.AWS_IOT_THING_NAME;
let intervalSpy: ReturnType<typeof vi.spyOn>;
let stdoutSpy: ReturnType<typeof vi.spyOn>;
let exitSpy: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  vi.resetModules();
  subscribeHandler = undefined;
  connectMock.mockReset();
  connectMock.mockResolvedValue({ kind: "fake-provider" });
  subscribeMock.mockClear();
  replyMock.mockClear();
  unsubscribeMock.mockClear();
  disconnectMock.mockClear();
  // Never actually start the keep-alive timer.
  intervalSpy = vi.spyOn(global, "setInterval").mockReturnValue(0 as unknown as NodeJS.Timeout);
  stdoutSpy = vi.spyOn(process.stdout, "write").mockReturnValue(true);
  exitSpy = vi.spyOn(process, "exit").mockImplementation(((): never => undefined as never) as never);
});

afterEach(() => {
  process.argv = origArgv;
  if (origThing === undefined) delete process.env.AWS_IOT_THING_NAME;
  else process.env.AWS_IOT_THING_NAME = origThing;
  process.removeAllListeners("SIGTERM");
  process.removeAllListeners("SIGINT");
  intervalSpy.mockRestore();
  stdoutSpy.mockRestore();
  exitSpy.mockRestore();
  vi.restoreAllMocks();
});

/** Import the harness fresh (it runs `main()` on import) and await it to settle. */
async function loadHarness(): Promise<void> {
  await import("../src/config_provider");
  // main() is async; let its connect/subscribe microtasks drain.
  await new Promise((r) => setTimeout(r, 0));
}

describe("config_provider harness", () => {
  it("connects, subscribes on the default topic, and announces readiness", async () => {
    process.argv = ["node", "config_provider.js"]; // no consumer arg, no thing env
    delete process.env.AWS_IOT_THING_NAME;
    await loadHarness();

    expect(connectMock).toHaveBeenCalledWith({ receiveOwnMessages: false });
    expect(subscribeMock).toHaveBeenCalledTimes(1);
    const topic = subscribeMock.mock.calls[0][0] as string;
    // Defaults: thing=lab-5950x, consumer=com.ggcommons.TsGgVerify.
    expect(topic).toBe("ggcommons/lab-5950x/config/get/com.ggcommons.TsGgVerify");
    expect((stdoutSpy.mock.calls[0][0] as string)).toContain(`config provider ready on ${topic}`);
    expect(intervalSpy).toHaveBeenCalled();
  });

  it("builds the request topic from the consumer arg and AWS_IOT_THING_NAME", async () => {
    process.argv = ["node", "config_provider.js", "com.example.Consumer"];
    process.env.AWS_IOT_THING_NAME = "edge-thing-7";
    await loadHarness();

    const topic = subscribeMock.mock.calls[0][0] as string;
    expect(topic).toBe("ggcommons/edge-thing-7/config/get/com.example.Consumer");
  });

  it("replies to a GetConfiguration request with the served config document", async () => {
    process.argv = ["node", "config_provider.js", "com.example.Consumer"];
    process.env.AWS_IOT_THING_NAME = "t1";
    await loadHarness();

    expect(subscribeHandler).toBeDefined();
    const fakeRequest = { header: { name: "GetConfiguration" } } as unknown as Message;
    subscribeHandler!("ignored-topic", fakeRequest);
    await new Promise((r) => setTimeout(r, 0));

    expect(replyMock).toHaveBeenCalledTimes(1);
    const [reqArg, replyMsg] = replyMock.mock.calls[0] as [Message, Message];
    expect(reqArg).toBe(fakeRequest);
    // The reply carries the config document with the expected sections.
    const body = (replyMsg as Message).getBody() as Record<string, unknown>;
    expect(body).toHaveProperty("logging");
    expect(body).toHaveProperty("heartbeat");
    expect(body).toHaveProperty("metricEmission");
    expect(body).toHaveProperty("tags");
    expect(body).toHaveProperty("component");
    expect((body.logging as Record<string, unknown>).level).toBe("INFO");
  });

  it("SIGTERM unsubscribes, disconnects (in order), then exits", async () => {
    process.argv = ["node", "config_provider.js", "com.example.Consumer"];
    process.env.AWS_IOT_THING_NAME = "t1";
    await loadHarness();

    const order: string[] = [];
    unsubscribeMock.mockImplementation(async () => {
      order.push("unsubscribe");
    });
    disconnectMock.mockImplementation(async () => {
      order.push("disconnect");
    });

    process.emit("SIGTERM");
    await new Promise((r) => setTimeout(r, 0));

    const topic = subscribeMock.mock.calls[0][0] as string;
    expect(unsubscribeMock).toHaveBeenCalledWith(topic);
    expect(disconnectMock).toHaveBeenCalledTimes(1);
    expect(order).toEqual(["unsubscribe", "disconnect"]);
    expect(exitSpy).toHaveBeenCalledWith(0);
  });

  it("SIGINT also triggers shutdown, and shutdown is idempotent across signals", async () => {
    process.argv = ["node", "config_provider.js"];
    await loadHarness();

    process.emit("SIGINT");
    await new Promise((r) => setTimeout(r, 0));
    expect(unsubscribeMock).toHaveBeenCalledTimes(1);
    expect(exitSpy).toHaveBeenCalledTimes(1);

    // A second signal must not unsubscribe/disconnect/exit again (guarded by shuttingDown).
    process.emit("SIGTERM");
    await new Promise((r) => setTimeout(r, 0));
    expect(unsubscribeMock).toHaveBeenCalledTimes(1);
    expect(disconnectMock).toHaveBeenCalledTimes(1);
    expect(exitSpy).toHaveBeenCalledTimes(1);
  });

  it("still exits even if disconnect rejects during shutdown (finally runs)", async () => {
    process.argv = ["node", "config_provider.js"];
    await loadHarness();

    // Real process.exit halts the event loop; emulate that here so the post-finally
    // re-raise of the disconnect error never escapes (it wouldn't in production either).
    exitSpy.mockImplementation(((): never => {
      throw new Error("__exit__");
    }) as never);
    // Swallow the simulated exit so it does not surface as an unhandled rejection.
    const onUnhandled = (e: unknown): void => {
      if (!(e instanceof Error) || e.message !== "__exit__") throw e;
    };
    process.once("unhandledRejection", onUnhandled);

    disconnectMock.mockRejectedValueOnce(new Error("disconnect failed"));
    // The shutdown's try/finally must still call process.exit(0) on a disconnect error.
    process.emit("SIGTERM");
    await new Promise((r) => setTimeout(r, 0));
    expect(exitSpy).toHaveBeenCalledWith(0);
    process.removeListener("unhandledRejection", onUnhandled);
  });
});
