/**
 * Small typed controller for one live coordination resource (list or
 * detail). Pure logic, no React: the hook owns timers/foreground gating and
 * drives `pollOnce` sequentially, while the controller owns in-flight
 * overlap protection, generation-guarded cancellation and closed error
 * mapping. Late results after `cancel`/`dispose` never publish.
 */
import { RelayRpcError } from "../transport/relayRpcClient";

export type CoordinationResourceState<T> =
  | { readonly status: "loading" }
  | { readonly status: "live"; readonly result: T }
  | {
    readonly status: "unavailable";
    readonly reason: string;
    readonly action: "retry" | "reconnect";
  };

export const COORDINATION_UNREACHABLE_MESSAGE = "Live coordination is unreachable right now.";
export const COORDINATION_SIGN_IN_MESSAGE = "Pair this phone to view live coordination.";

/** Closed error mapping: login surfaces sign-in, malformed stays unreadable. */
export function coordinationReasonOf(error: unknown): string {
  if (error instanceof RelayRpcError) {
    if (error.loginRequired) return COORDINATION_SIGN_IN_MESSAGE;
    return error.message || COORDINATION_UNREACHABLE_MESSAGE;
  }
  return COORDINATION_UNREACHABLE_MESSAGE;
}

export type CoordinationFetch<T> = () => Promise<T>;
export type CoordinationPublish<T> = (state: CoordinationResourceState<T>) => void;

/**
 * One controller per owner generation. The hook recreates it whenever the
 * owner (account/device/team/rpc client/socket epoch/retry) changes; the old
 * controller is disposed so its late callbacks never feed the new record.
 */
export class CoordinationResourceController<T> {
  private generation = 0;
  private inFlight = false;
  private disposed = false;

  constructor(
    private readonly fetch: CoordinationFetch<T>,
    private readonly publish: CoordinationPublish<T>,
  ) {}

  /** Overlap guard for tests: true while a fetch is unresolved. */
  get fetching(): boolean {
    return this.inFlight;
  }

  /**
   * One sequential poll step. No overlapping fetches; results publish only
   * when this generation is still current. Failures surface `unavailable`
   * with a retry action — never a false empty.
   */
  async pollOnce(): Promise<void> {
    if (this.disposed || this.inFlight) return;
    const generation = this.generation;
    this.inFlight = true;
    try {
      const result = await this.fetch();
      if (!this.disposed && this.generation === generation) {
        this.publish({ status: "live", result });
      }
    } catch (error) {
      if (!this.disposed && this.generation === generation) {
        this.publish({
          status: "unavailable",
          reason: coordinationReasonOf(error),
          action: "retry",
        });
      }
    } finally {
      this.inFlight = false;
    }
  }

  /** Invalidate the in-flight fetch without publishing: late results drop. */
  cancel(): void {
    this.generation += 1;
  }

  /** Release the controller: no further fetch or publish ever happens. */
  dispose(): void {
    this.disposed = true;
    this.generation += 1;
  }
}
