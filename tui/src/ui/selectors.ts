import type { OverlayHandle, SizeValue } from "@earendil-works/pi-tui";
import type { AppServerHost } from "../app-server/host.ts";
import type { AppServerSession } from "../app-server/session.ts";
import { AppServerRequestError, isDeliveryCancelled } from "../app-server/client.ts";
import { TransportClosedError } from "../app-server/transport.ts";
import {
  DeliveryResidueError,
  DeliveryUncertainError,
  deliveryDestination,
  localEffect,
  openDelivery,
  residueText,
  saveDelivery,
  type LocalEffect,
  type Opener,
} from "../app-server/delivery-files.ts";
import type { CommandDispatcher } from "../commands/dispatcher.ts";
import type { CommandOutcome } from "../commands/outcome.ts";
import type { DeliveryPage } from "../presentation/deliveries.ts";
import type { CatalogModelView, SessionSummaryView, SessionUserMessageBoundaryObservation, SessionView } from "../protocol/app-server.ts";
import { sanitizeField } from "../sanitize.ts";
import type { SessionCatalogPage } from "../startup.ts";
import { DeliverySelector, type DeliveryAvailability } from "./components/delivery-selector.ts";
import { ModelSelector } from "./components/model-selector.ts";
import type { PopupContent } from "./components/popup-frame.ts";
import { ResumeSelector } from "./components/resume-selector.ts";
import { BoundarySelector } from "./components/session-selector.ts";
import { TreeSelector, type TreeSelection } from "./components/tree-selector.ts";
import { compactDiagnostic } from "../diagnostic-text.ts";
import type { PresentationLease } from "./presentation-lease.ts";
import type { SessionDeletionWorkflow } from "./session-deletion-workflow.ts";

/** What the selector surfaces read from and ask of the terminal app. */
export interface SelectorPort {
  lease(): PresentationLease;
  isCurrent(lease: PresentationLease): boolean;
  host(): AppServerHost;
  session(): AppServerSession | undefined;
  dispatcher(): CommandDispatcher;
  deletion(): SessionDeletionWorkflow;
  switching(): boolean;
  /** The app's single popup slot. */
  overlay(): OverlayHandle | undefined;
  humanInputOverlay(): unknown;
  showPopup(content: PopupContent, options: { width: SizeValue; heightPercent: number; minWidth?: number }, retire?: () => void): OverlayHandle;
  closeOverlay(): void;
  requestRender(): void;
  showTransient(level: "info" | "error", text: string): void;
  showInspection(title: string, body: string, lease: PresentationLease): void;
  handleOutcome(outcome: CommandOutcome, lease: PresentationLease): Promise<void>;
  /** The resume selector now presented in the popup slot. */
  presentResume(selector: ResumeSelector): void;
}

/**
 * The terminal's selection surfaces: model catalog, Session resume, fork and
 * branch boundaries, the Session tree, and committed deliveries. Each opens
 * in the app's single popup slot under one presentation lease, and turns a
 * selection into the same command outcome a typed command would produce.
 */
export class SelectorSurfaces {
  readonly #port: SelectorPort;
  readonly #opener: { command: string | undefined; launch: Opener };
  constructor(port: SelectorPort, opener: { command: string | undefined; launch: Opener }) { this.#port = port; this.#opener = opener; }
  /**
   * Opens approval selection over the editor without changing runtime policy.
   *
   * The overlay owns focus while it is up and hands it straight back to the
   * editor on select or cancel, so the editor is never left unfocused.
   */
  showModelSelector(models: CatalogModelView[], lease: PresentationLease): void {
    const attachedSession = lease.session;
    if (attachedSession === undefined) return;
    if (!this.#port.isCurrent(lease)) return;
    const state = attachedSession.state;
    if (state === undefined) return;
    if (state.sessionModel === null) {
      this.#port.showInspection('Model catalog', `${attachedSession.catalogDeferredNotice ?? 'Session model configuration is deferred.'}\nUse /model-position for a deliberate native-defaults selection.`, lease);
      return;
    }
    const selector = new ModelSelector({
      models,
      sessionModel: state.sessionModel,
      attempt: state.attempt,
      deferredNotice: attachedSession.catalogDeferredNotice,
    });
    const handle = this.#port.showPopup(selector, { width: "80%", heightPercent: 70 });

    const close = () => {
      if (this.#port.overlay() !== handle) return;
      this.#port.closeOverlay();
    };

    selector.onChange = () => {
      if (this.#port.isCurrent(lease)) this.#port.requestRender();
    };
    selector.onCancel = () => {
      if (this.#port.isCurrent(lease)) close();
    };
    const nextOffset=attachedSession.nextModelOffset;
    if(nextOffset!==undefined)selector.onNextPage=()=>{
      if(!this.#port.isCurrent(lease)||this.#port.overlay()!==handle)return;
      void attachedSession.modelCatalog(nextOffset).then(catalog=>{if(this.#port.isCurrent(lease)&&this.#port.overlay()===handle)this.showModelSelector(catalog.models??[],lease);}).catch(error=>{if(this.#port.isCurrent(lease))this.#port.showTransient('error',String(error));});
    };
    selector.onSelect = (model) => {
      if (!this.#port.isCurrent(lease)) return;
      close();
      void this.#port.dispatcher().selectModel(model)
        .then((outcome) => this.#port.handleOutcome(outcome, lease))
        .catch((error: unknown) => {
          if (this.#port.isCurrent(lease)) {
            this.#port.showTransient("error", `model selection failed: ${compactDiagnostic(error)}`);
          }
        });
    };
  }

  async openResumeSelector(page?: SessionCatalogPage): Promise<void> {
    const lease = this.#port.lease();
    try {
      const catalog = page ?? await this.#port.host().listSessions();
      if (!this.#port.isCurrent(lease)) return;
      this.showSessionSelector(catalog.sessions, catalog.nextOffset, "", lease);
    } catch (error) {
      if (this.#port.isCurrent(lease)) {
        this.#port.showTransient("error", `session list unavailable: ${compactDiagnostic(error)}. Enter retries; Ctrl+C exits.`);
      }
    }
  }

  showSessionSelector(
    sessions: SessionSummaryView[] | undefined,
    nextOffset: number | undefined,
    query: string,
    lease: PresentationLease,
  ): void {
    if (!this.#port.isCurrent(lease)) return;
    if (this.#port.humanInputOverlay() !== undefined) return;
    const workflow = this.#port.deletion();
    // Initial /resume command responses obey the same mutation boundary as pages.
    if (lease.sessionListGeneration !== workflow.generation) return;
    if (this.#port.session() !== undefined && sessions?.length === 0 && workflow.state.kind === "idle") {
      this.#port.showTransient("info", "no persisted sessions are available");
      return;
    }
    const selector = new ResumeSelector({
      initialPage: sessions === undefined ? undefined : { sessions, nextOffset }, query, client: this.#port.host(), workflow,
      alive: () => this.#port.isCurrent(lease) && this.#port.overlay() === handle,
      feedback: (text) => this.#port.showTransient("info", text),
    });
    const handle = this.#port.showPopup(selector, { width: "80%", heightPercent: 70 });
    this.#port.presentResume(selector);
    selector.onChange = () => {
      if (this.#port.isCurrent(lease)) this.#port.requestRender();
    };
    selector.onCancel = () => {
      if (this.#port.isCurrent(lease) && this.#port.overlay() === handle) this.#port.closeOverlay();
    };
    if (this.#port.session() === undefined) {
      selector.onCreate = async () => {
        if (!this.#port.isCurrent(lease) || this.#port.overlay() !== handle || this.#port.switching()) return;
        const outcome = await this.#port.dispatcher().newSession();
        if (!this.#port.isCurrent(lease)) return;
        // A committed create invalidates the empty catalog even if attach fails.
        // Retire its action now; reopening resume must read fresh authority.
        if (outcome.kind === "focus_session") this.#port.closeOverlay();
        await this.#port.handleOutcome(outcome, lease);
      };
    }
    selector.onSelect = (session) => {
      if (!this.#port.isCurrent(lease)) return;
      void this.#port.handleOutcome(
        this.#port.dispatcher().selectSession(session.id),
        lease,
      ).catch((error: unknown) => {
        if (this.#port.isCurrent(lease)) {
          this.#port.showTransient("error", `session selection failed: ${compactDiagnostic(error)}`);
        }
      });
    };
  }

  showBoundarySelector(
    boundaries: SessionUserMessageBoundaryObservation[],
    title: string,
    operation: "fork" | "tree",
    lease: PresentationLease,
    nextOffset?: number,
  ): void {
    const attachedSession = lease.session;
    if (attachedSession === undefined) return;
    if (!this.#port.isCurrent(lease)) return;
    if (boundaries.length === 0) {
      this.#port.showTransient(
        "info",
        "the active lineage has no committed user-message boundary",
      );
      return;
    }
    let currentNextOffset = nextOffset;
    const selector = new BoundarySelector({ boundaries, title, nextOffset });
    const handle = this.#port.showPopup(selector, { width: "80%", heightPercent: 70 });
    selector.onChange = () => {
      if (this.#port.isCurrent(lease)) this.#port.requestRender();
    };
    selector.onCancel = () => {
      if (this.#port.isCurrent(lease) && this.#port.overlay() === handle) {
        this.#port.closeOverlay();
      }
    };
    selector.onLoadMore = () => {
      const offset = currentNextOffset;
      if (offset === undefined) return;
      void attachedSession.boundaries(offset).then((page) => {
        if (!this.#port.isCurrent(lease)) return;
        currentNextOffset = page.nextOffset;
        selector.appendPage(page.boundaries, page.nextOffset);
      }).catch((error: unknown) => {
        if (!this.#port.isCurrent(lease)) return;
        this.#port.showTransient("error", `history page failed: ${compactDiagnostic(error)}`);
        selector.appendPage([], currentNextOffset);
      });
    };
    selector.onSelect = (boundary) => {
      if (!this.#port.isCurrent(lease)) return;
      this.#port.closeOverlay();
      const request = operation === "fork"
        ? this.#port.dispatcher().forkAt(boundary)
        : this.#port.dispatcher().branchAt(boundary);
      void request
        .then((outcome) => this.#port.handleOutcome(outcome, lease))
        .catch((error: unknown) => {
          if (this.#port.isCurrent(lease)) {
            this.#port.showTransient("error", `session switch failed: ${compactDiagnostic(error)}`);
          }
        });
    };
  }

  showTreeSelector(
    session: SessionView,
    nodes: import("../protocol/app-server.ts").SessionNodeView[],
    nextNodeOffset: number | undefined,
    boundaries: SessionUserMessageBoundaryObservation[],
    nextHistoryOffset: number | undefined,
    lease: PresentationLease,
  ): void {
    const attachedSession = lease.session;
    if (attachedSession === undefined) return;
    if (!this.#port.isCurrent(lease)) return;
    const selector = new TreeSelector({
      session,
      nodes,
      nextNodeOffset,
      boundaries,
      nextHistoryOffset,
    });
    const handle = this.#port.showPopup(selector, { width: "80%", heightPercent: 70 });
    selector.onChange = () => {
      if (this.#port.isCurrent(lease)) this.#port.requestRender();
    };
    selector.onCancel = () => {
      if (this.#port.isCurrent(lease) && this.#port.overlay() === handle) {
        this.#port.closeOverlay();
      }
    };
    selector.onLoadMore = () => {
      const request = selector.nextPageRequest();
      if (request === undefined) return;
      // The graph page and the boundary page come from their own owners.
      void Promise.all([
        this.#port.host().sessionTree(session.id, request.nodeOffset),
        attachedSession.boundaries(request.historyOffset),
      ]).then(([tree, page]) => {
        if (!this.#port.isCurrent(lease)) return;
        selector.appendPage({
          nodes: tree.nodes,
          nextNodeOffset: tree.nextOffset,
          boundaries: page.boundaries,
          nextHistoryOffset: page.nextOffset,
        });
      }).catch((error: unknown) => {
        if (!this.#port.isCurrent(lease)) return;
        this.#port.showTransient("error", `tree page failed: ${compactDiagnostic(error)}`);
        selector.retryPage();
      });
    };
    selector.onSelect = (selection: TreeSelection) => {
      if (!this.#port.isCurrent(lease)) return;
      this.#port.closeOverlay();
      const outcome = selection.kind === "node"
        ? Promise.resolve(
            this.#port.dispatcher().selectTreeNode(session.id, selection.node.id),
          )
        : this.#port.dispatcher().branchAt(selection.boundary);
      void outcome
        .then((resolved) => this.#port.handleOutcome(resolved, lease))
        .catch((error: unknown) => {
          if (this.#port.isCurrent(lease)) {
            this.#port.showTransient("error", `session focus change failed: ${compactDiagnostic(error)}`);
          }
        });
    };
  }

  /**
   * `/files`: committed deliveries and explicit client-local actions.
   *
   * Availability comes from facts established at composition, never from
   * paths: delivery access is the connection's transport grant, and a shared
   * filesystem requires that this TUI spawned the server (Open additionally
   * verifies the leaf identity before launching anything). Each action is
   * one owned operation under the interaction's owner: retiring the surface
   * aborts it, and only an effect that already happened outlives it.
   */
  showDeliverySelector(page: DeliveryPage, lease: PresentationLease): void {
    const session = lease.session;
    if (session === undefined || !this.#port.isCurrent(lease)) return;
    const sharedHost = this.#port.host().ownership === "owned_child";
    const opener = this.#opener.command;
    const launch = this.#opener.launch;
    const noAccess = this.#port.host().ownership === "owned_child"
      ? "the App Server did not grant delivery access to this connection"
      : "this connection holds no delivery access; reconnect with --delivery-access-token-file";
    const availability: DeliveryAvailability = {
      save: !session.deliveryAccess ? noAccess : undefined,
      open: !session.deliveryAccess ? noAccess
        : !sharedHost ? "this App Server's files are not on this machine; use Save"
          : opener === undefined ? "no system opener on this platform" : undefined,
    };
    const selector = new DeliverySelector(page, availability);
    // The one owner of this interaction's work. Every way it ends — Escape,
    // another overlay replacing it, Session focus change, attachment or
    // snapshot replacement, reconnect, terminal failure, quit — closes the
    // overlay, and closing aborts it: uncommitted local effects stop and the
    // native read or locate is cancelled on the server.
    const interaction = new AbortController();
    const handle = this.#port.showPopup(selector, { width: "80%", heightPercent: 70 }, () => interaction.abort());
    let inFlight: { operation: number; abort: AbortController } | undefined;
    const current = () => this.#port.isCurrent(lease) && this.#port.overlay() === handle;
    selector.onChange = () => {
      if (current()) this.#port.requestRender();
    };
    selector.onCancel = () => {
      if (current()) this.#port.closeOverlay();
    };
    selector.onAbort = (operation) => {
      if (inFlight?.operation === operation) inFlight.abort.abort();
    };
    selector.onLoadMore = () => {
      const before = selector.nextCursor;
      if (before === undefined) return;
      void session.deliveryPage(before, selector.readCut).then((older) => {
        if (current()) selector.appendPage(older);
      }).catch((error: unknown) => {
        if (!current()) return;
        this.#port.showTransient("error", `delivery history page failed: ${compactDiagnostic(error)}`);
        selector.retryPage();
      });
    };
    selector.onLoadCollection = (collection) => {
      void session.deliveryCollection(collection).then((next) => {
        if(current()) selector.replaceCollectionPage(next);
      }).catch((error:unknown)=>{
        if(!current())return;
        this.#port.showTransient("error", `delivery collection failed: ${compactDiagnostic(error)}`);
        selector.retryPage();
      });
    };
    selector.onAction = (operation, action, record, destination) => {
      const abort = new AbortController();
      const signal = AbortSignal.any([interaction.signal, abort.signal]);
      inFlight = { operation, abort };
      // What this action has done outside the process, recorded by the
      // action at the moment it happens.
      const effect = localEffect();
      const task = (async () => {
        if(action === "metadata") {
          const file = await session.deliveryMetadata(record, () => current() && !signal.aborted);
          if(current() && !signal.aborted) selector.installMetadata(operation,record,file);
          return "Original delivery metadata loaded";
        }
        if(record.file == null) throw new Error("Read original delivery metadata first.");
        if (action === "save") {
          const saved = await saveDelivery(
            () => session.readDelivery(record, signal),
            deliveryDestination(destination ?? ""),
            signal,
            undefined,
            effect,
          );
          const text = `Saved ${record.file.name} to ${saved.path}`;
          return saved.residue === undefined ? text
            : `${text}; warning: ${residueText(saved.residue.path, saved.residue.staged)} (${compactDiagnostic(saved.residue.cause)})`;
        }
        const requested = await openDelivery(() => session.locateDelivery(record, signal), {
          sharedHost,
          opener,
          launch,
          signal,
          effect,
        });
        return `${requested.opener} accepted the request to open ${requested.path}`;
      })();
      // One terminal report per operation. A live surface shows every
      // outcome. Once the interaction is retired, an outcome is still owed
      // only if the action committed an external effect (a dispatched save
      // link or a spawned opener) or left a staged file behind: it is reported
      // once, on the transient surface, never into a successor surface. A
      // cancelled or failed action that did neither is not reported.
      const report = (level: "info" | "error", text: string) => {
        const safe = sanitizeField(text);
        if (current()) selector.settle(operation, level, safe);
        else if (effect.committed || effect.residue) this.#port.showTransient(level, safe);
      };
      void task.then(
        (text) => report("info", text),
        (error: unknown) => report("error", deliveryFailure(error, signal, effect)),
      ).finally(() => {
        if (inFlight?.operation === operation) inFlight = undefined;
      });
    };
  }

}

/** One bounded line for a failed or cancelled delivery action. */
function deliveryFailure(error: unknown, signal: AbortSignal, effect: LocalEffect): string {
  if (error instanceof DeliveryResidueError || error instanceof DeliveryUncertainError) return error.message;
  // Cancellation won before Save's publication admission, Open's launch, or
  // the server's response publication. A committed effect is never
  // described as nothing having happened, whatever aborted afterwards.
  if (!effect.committed && ((signal.aborted && error === signal.reason) || isDeliveryCancelled(error))) {
    return "Cancelled; nothing was saved or opened";
  }
  if (error instanceof TransportClosedError) return "Disconnected from the App Server; nothing was written";
  if (error instanceof AppServerRequestError && error.error.data?.kind === "session_file_read") {
    const reason = error.error.data.reason;
    const text: Record<typeof reason, string> = {
      missing: "the delivered file no longer exists",
      unauthorized: "this connection is not authorized to access it",
      unavailable: "its original Session or root is unavailable",
      not_regular: "it is no longer a regular file",
      replaced: "it was replaced during the operation; try again",
      too_large: "it exceeds the 512 KiB transfer limit",
      capacity: "the App Server is busy with other file reads; try again",
      read_failed: "the App Server could not read it",
    };
    return `Failed: ${text[reason]}`;
  }
  return `Failed: ${compactDiagnostic(error)}`;
}
