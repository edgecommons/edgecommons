declare module "@edgecommons/streamlog-node" {
  export interface LogEvent {
    level: number;
    target: string;
    message: string;
  }

  export class StreamHandle {
    append(partitionKey: string, timestampMs: number, payload: Buffer): void;
    flush(): void;
  }

  export class StreamService {
    static open(configJson: string): StreamService;
    stream(name: string): StreamHandle;
    stats(name: string): any;
    close(): void;
  }

  export function setLogCallback(cb: (err: Error | null, ev: LogEvent) => void): void;
  export function registerSinkCallback(
    streamName: string,
    cb: (err: Error | null, arg: [number, unknown[]]) => void,
  ): void;
  export function resolveOutcome(batchId: number, code: number, failedOffsets?: number[] | null): void;
}
