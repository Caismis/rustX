import { validateInteractionBody } from "../../../protocol/app-server/interaction.ts";
import type { CheckpointPage, RuntimeClientInteractionHeader } from "../../../protocol/app-server/v57.ts";
import type { SizeValue } from "@earendil-works/pi-tui";
import type { AppServerSession } from "../app-server/session.ts";
import type { PreferenceChange } from "../commands/outcome.ts";
import { displayInteraction, hasInteractionBody, type DisplayInteraction } from "../presentation/interaction-display.ts";
import { findPendingInteraction, reconcileInteractionFocus, sameInteractionRef } from "../presentation/interaction-focus.ts";
import type { PresentationState } from "../presentation/state.ts";
import type { InteractionRef, InteractionResponse, RoutedInteraction } from "../protocol/app-server.ts";
import { HumanInteractionOverlay } from "./components/hitl.ts";
import type { PopupContent } from "./components/popup-frame.ts";
import type { PresentationLease } from "./presentation-lease.ts";
import { compactDiagnostic } from "../diagnostic-text.ts";
import { interactionKey, type PresentationPreferences } from "./preferences.ts";

/** What the human-input controller reads from and asks of the terminal app. */
export interface HumanInputPort {
  session(): AppServerSession | undefined;
  lease(): PresentationLease;
  isCurrent(lease: PresentationLease): boolean;
  render(state: PresentationState): void;
  requestRender(): void;
  showTransient(level: "info" | "error", text: string): void;
  showPopup(content: PopupContent, options: { width: SizeValue; heightPercent: number; minWidth?: number }): void;
  closeOverlay(): void;
  interrupt(): void;
  applyPreference(change: PreferenceChange): void;
  preferences(): PresentationPreferences;
}

/**
 * The terminal's human-in-the-loop interaction surface.
 *
 * Presentation-only ownership of which pending interaction is focused, the
 * bounded page and selected body/argument ranges read for it, and the
 * dismissal marker. The runtime remains the only owner of the interactions
 * themselves: every response names the exact `InteractionRef` it was
 * collected for, and every asynchronous read is fenced by the app's
 * presentation lease and the native pending-interaction revision.
 */
export class HumanInputController {
  readonly #port: HumanInputPort;
  constructor(port: HumanInputPort) { this.#port = port; }
  #overlay: HumanInteractionOverlay | undefined;
  /**
   * Presentation-only focus over `pendingInteractions`, reconciled against
   * the authoritative projection on every render. Never a semantic fact: it
   * picks which interaction the human-input surface shows, nothing else.
   */
  #focus: InteractionRef | undefined;
  #page: { session: AppServerSession; stamp: string; offset: number; page: CheckpointPage; items: DisplayInteraction[] } | undefined;
  #pageRead = 0;
  #body?: {session:AppServerSession;stamp:string;interaction:InteractionRef;body:RoutedInteraction};
  #bodyRead = 0;
  #argumentRange?: {session:AppServerSession;stamp:string;interaction:InteractionRef;text:string;offset:number;next:number|null;end:number;field:"arguments"|"reason"|"tool_name";decoder:InstanceType<typeof TextDecoder>};
  /**
   * The interaction the user dismissed the human-input surface on (approval
   * Esc), plus the pending queue exactly as it stood at dismissal.
   *
   * Presentation-only: the dismissed interaction stays pending and
   * unanswered, and the surface reopens on Ctrl+G. The dismissal is scoped
   * to that exact interaction and that exact queue: the moment a *distinct*
   * interaction arrives, the dismissal is superseded — a previously
   * dismissed approval must never silently hide newly required human input.
   * Settlement or removal of the dismissed interaction retires the marker.
   */
  #dismissed:
    | { interaction: InteractionRef; known: ReadonlySet<string> }
    | undefined;
  #stamp(): string {
    const cut = this.#port.session()?.checkpoint?.read_cut;
    return cut?.domain_revisions['pending_interactions'] ?? cut?.revision ?? '';
  }

  #total(): bigint {
    const frontier = this.#port.session()?.checkpoint?.collections.find(value => value.domain === 'pending_interactions');
    return BigInt(this.#page?.page.total ?? frontier?.total ?? this.#port.session()?.state.pendingInteractions.length ?? 0);
  }

  #partial(): boolean {
    return Boolean(this.#page?.offset) || this.#total() > BigInt(this.#page?.items.length ?? this.#port.session()?.state.pendingInteractions.length ?? 0);
  }

  #pageLabel(): string | undefined {
    if (!this.#partial()) return undefined;
    const offset = this.#page?.offset ?? 0;
    const total = this.#total();
    const end = BigInt(offset + (this.#page?.items.length ?? this.#port.session()?.state.pendingInteractions.length ?? 0));
    return `Pending ${offset + 1}–${end < total ? end : total} of ${total} · Ctrl+PgUp/PgDn pages`;
  }

  async #pageTo(direction: -1 | 1): Promise<void> {
    const session = this.#port.session();
    if (!session || typeof session.checkpointPage !== 'function') return;
    const previous = this.#page;
    if(direction<0 && (previous?.offset ?? 0)===0) return;
    const offset = direction < 0 ? Math.max(0, (previous?.offset ?? 0) - 32)
      : Number(previous?.page.next_offset ?? (previous ? previous.offset + previous.items.length : session.state.pendingInteractions.length));
    if (!Number.isSafeInteger(offset) || offset < 0 || BigInt(offset) >= this.#total()) return;
    const lease = this.#port.lease(), overlay = this.#overlay, stamp = this.#stamp(), read = ++this.#pageRead;
    try {
      const { page, items } = await session.checkpointPage('pending_interactions', offset, previous?.page);
      if (!overlay || this.#overlay !== overlay || !this.#port.isCurrent(lease) || read !== this.#pageRead || stamp !== this.#stamp()) return;
      if (page.items.length > 32) throw new Error('Pending interaction page exceeded its item bound.');
      this.#page = { session, stamp, offset, page, items: (items as RuntimeClientInteractionHeader[]).map(displayInteraction) };
      this.#focus = this.#page.items[0]?.interaction;
      this.#dismissed = undefined;
      this.#port.render(session.state);
    } catch (error) {
      if (overlay && this.#overlay === overlay && this.#port.isCurrent(lease) && read === this.#pageRead) {
        this.#port.showTransient('error', `Pending interactions unavailable: ${compactDiagnostic(error)}`);
      }
    }
  }

  async #readArguments(interaction: InteractionRef, next: boolean, selectedField: "arguments"|"reason"|"tool_name" = "arguments"): Promise<void> {
    const session=this.#port.session(), overlay=this.#overlay;
    if(!session || !overlay) return;
    const rows=this.#page?.items ?? session.state.pendingInteractions;
    const index=rows.findIndex(item=>sameInteractionRef(item.interaction,interaction));
    if(index<0) return;
    const item=rows[index]!;
    if(hasInteractionBody(item) || item.kind!=='approval') return;
    const stamp=this.#stamp(), old=this.#argumentRange;
    const previous=old?.session===session && old.stamp===stamp && sameInteractionRef(old.interaction,interaction) ? old : undefined;
    const offset=next ? previous?.next : 0;
    const field=next ? previous?.field ?? selectedField : selectedField;
    if(offset==null) return;
    const lease=this.#port.lease(), generation=++this.#bodyRead;
    const current=()=>this.#overlay===overlay && this.#port.isCurrent(lease) && generation===this.#bodyRead && stamp===this.#stamp() && sameInteractionRef(this.#focus ?? interaction,interaction);
    try {
      const chunk=await session.interactionValue(interaction,Number(this.#page?.page.items[index]?.offset ?? item.source_offset),this.#page?.page,offset,['request','kind',field]);
      if(!current()) return;
      if(chunk.bytes_base64.length>87_384 || chunk.bytes_base64.length%4!==0 || chunk.bytes_base64.length/4*3-(chunk.bytes_base64.endsWith('==')?2:chunk.bytes_base64.endsWith('=')?1:0)>65_536)throw new Error('Encoded native range exceeds the decoded byte bound.');
      const bytes=Uint8Array.from(atob(chunk.bytes_base64),character=>character.charCodeAt(0));
      if(bytes.length>65536 || !Number.isSafeInteger(chunk.readable_end) || offset+bytes.length>chunk.readable_end || (offset<chunk.readable_end && !bytes.length) || (chunk.next_offset ?? null)!==(offset+bytes.length<chunk.readable_end?offset+bytes.length:null) || (previous && next && previous.end!==chunk.readable_end)) throw new Error('Argument range frontier changed.');
      const decoder=previous && next ? previous.decoder : new TextDecoder('utf-8',{fatal:true,ignoreBOM:true});
      this.#argumentRange={session,stamp,interaction,text:decoder.decode(bytes,{stream:chunk.next_offset!=null}),offset,field,next:chunk.next_offset??null,end:chunk.readable_end,decoder};
      this.#port.requestRender();
    } catch(error){if(current())this.#port.showTransient('error',`Arguments unavailable: ${compactDiagnostic(error)}`);}
  }

  async #loadBody(interaction: InteractionRef): Promise<void> {
    const session=this.#port.session(), overlay=this.#overlay;
    if(!session || !overlay) return;
    const rows=this.#page?.items ?? session.state.pendingInteractions;
    const index=rows.findIndex(item=>sameInteractionRef(item.interaction,interaction));
    if(index<0 || hasInteractionBody(rows[index]!)) return;
    const selected=rows[index]!;
    if(hasInteractionBody(selected))return;
    const ordinal=Number(this.#page?.page.items[index]?.offset ?? selected.source_offset);
    const lease=this.#port.lease(), stamp=this.#stamp(), generation=++this.#bodyRead;
    const current=()=>this.#overlay===overlay && this.#port.isCurrent(lease) && generation===this.#bodyRead && stamp===this.#stamp() && sameInteractionRef(this.#focus ?? interaction,interaction);
    try {
      let offset=0, total:number|undefined; const bytes:Uint8Array[]=[];
      do {
        const chunk=await session.interactionValue(interaction,ordinal,this.#page?.page,offset);
        if(!current()) return;
        if(!Number.isSafeInteger(chunk.readable_end) || chunk.readable_end>16*1024*1024 || (total!==undefined && total!==chunk.readable_end)) throw new Error('This body exceeds the selected-body cache; inspect bounded argument ranges.');
        total=chunk.readable_end;
        if(chunk.bytes_base64.length>87_384 || chunk.bytes_base64.length%4!==0 || chunk.bytes_base64.length/4*3-(chunk.bytes_base64.endsWith('==')?2:chunk.bytes_base64.endsWith('=')?1:0)>65_536)throw new Error('Encoded native range exceeds the decoded byte bound.');
        const value=Uint8Array.from(atob(chunk.bytes_base64),character=>character.charCodeAt(0));
        if(value.length>65536 || (offset<total && !value.length) || offset+value.length>total || (chunk.next_offset ?? null)!==(offset+value.length<total?offset+value.length:null)) throw new Error('Interaction range did not advance exactly.');
        bytes.push(value);offset+=value.length;
      } while(offset<total);
      const data=new Uint8Array(total);let at=0;for(const value of bytes){data.set(value,at);at+=value.length;}
      const body=JSON.parse(new TextDecoder('utf-8',{fatal:true,ignoreBOM:true}).decode(data)) as RoutedInteraction;
      const header=rows[index]!;
      if(hasInteractionBody(header)) return;
      validateInteractionBody(header,body);
      this.#body={session,stamp,interaction,body};this.#port.render(session.state);
    } catch(error) {if(current())this.#port.showTransient('error',`Interaction body unavailable: ${compactDiagnostic(error)}`);}
  }

  /**
   * Presents the unified human-input surface from authoritative state.
   *
   * One surface serves every pending routed interaction — approvals and
   * questionnaires, primary and subagent. It opens when the projection holds
   * a focused interaction the user has not dismissed, updates in place while
   * the projection evolves, and disappears when the interaction set empties
   * or the surface is dismissed. The runtime remains the only owner: closing
   * or dismissing this surface settles nothing, and a response always names
   * the exact `InteractionRef` it was collected for.
   */
  sync(state: PresentationState): void {
    // A dismissal is scoped to the exact dismissed interaction and to the
    // queue as it stood then. A distinct arrival supersedes the dismissal:
    // the newly required human input takes the focus (the surface was
    // dismissed, so no in-progress panel is interrupted) and the surface
    // reopens, while the dismissed interaction stays pending and unanswered.
    // The move itself is presentation-only — it emits no response.
    const dismissed = this.#dismissed;
    if (dismissed !== undefined) {
      if (
        !this.#partial() && findPendingInteraction(state.pendingInteractions, dismissed.interaction) === undefined
      ) {
        this.#dismissed = undefined;
      } else {
        const arrival = state.pendingInteractions.find(
          (entry) => !dismissed.known.has(interactionKey(entry.interaction)),
        );
        if (arrival !== undefined) {
          this.#dismissed = undefined;
          this.#focus = arrival.interaction;
        }
      }
    }
    const focused = this.#focus;
    if (focused === undefined) {
      if (this.#overlay !== undefined) this.#port.closeOverlay();
      return;
    }
    const existing = this.#overlay;
    if (existing !== undefined) {
      existing.update(state.pendingInteractions, focused, this.#port.preferences(), !this.#partial());
      this.#port.requestRender();
      return;
    }
    if (
      this.#dismissed !== undefined &&
      sameInteractionRef(this.#dismissed.interaction, focused)
    ) {
      return;
    }
    const lease = this.#port.lease();
    const overlay = new HumanInteractionOverlay({
      onReview: (interaction, response) => this.#respond(lease, overlay, interaction, { type: "review", response }),
      onDecision: (interaction, decision) =>
        this.#respond(lease, overlay, interaction, {
          type: "approval",
          decision,
        }),
      onQuestionnaireSubmit: (interaction, response) =>
        this.#respond(lease, overlay, interaction, {
          type: "questionnaire",
          response,
        }),
      onQuestionnaireDecline: (interaction) =>
        this.#respond(lease, overlay, interaction, {
          type: "questionnaire",
          response: { type: "declined" },
        }),
      onDismiss: (interaction) => {
        // Scope the dismissal to this exact interaction and to the queue as
        // it stands now, so a later distinct arrival supersedes it.
        this.#dismissed = {
          interaction,
          known: new Set(
            (this.#port.session()?.state?.pendingInteractions ?? []).map((entry) =>
              interactionKey(entry.interaction),
            ),
          ),
        };
        if (this.#overlay === overlay) {
          this.#port.closeOverlay();
        }
      },
      onInterrupt: () => void this.#port.interrupt(),
      onNavigate: (interaction) => {
        // Navigation is presentation-only: it moves the focus and redraws.
        this.#focus = interaction;
        const current = this.#port.session()?.state;
        if (current !== undefined) this.#port.render(current);
      },
      onToggleExpand: (interaction) => {
        // Disclosure only: the same preference domain `/expand interaction`
        // uses, never a second approval gate.
        this.#port.applyPreference({ type: "expand_interaction", interaction });
      },
      onChange: () => this.#port.requestRender(),
      onPage: (direction) => { void this.#pageTo(direction); },
      pageLabel: () => this.#pageLabel(),
      onLoadBody: (interaction) => { void this.#loadBody(interaction); },
      onArguments: (interaction,next,field) => {void this.#readArguments(interaction,next,field);},
      argumentRange: (interaction) => {const range=this.#argumentRange;return range && range.session===this.#port.session() && range.stamp===this.#stamp() && sameInteractionRef(range.interaction,interaction) ? {label:`${range.field} bytes ${range.offset}–${range.next ?? range.end} of ${range.end}`,text:range.text}:undefined;},
    });
    overlay.update(state.pendingInteractions, focused, this.#port.preferences(), !this.#partial());
    this.#port.showPopup(overlay, { width: "94%", minWidth: 44, heightPercent: 90 });
    this.#overlay = overlay;
  }

  /**
   * The one typed response path for every interaction kind.
   *
   * Approval decisions and questionnaire responses both go through
   * `interaction_respond` with the exact routed identity the surface
   * collected them for. A rejection re-enables the panel that sent it —
   * routed by that same identity — so a failed response never double-sends
   * and never disturbs an unrelated pending interaction.
   */
  #respond(
    lease: PresentationLease,
    overlay: HumanInteractionOverlay,
    interaction: InteractionRef,
    response: InteractionResponse,
  ): void {
    const attachedSession = lease.session;
    if (attachedSession === undefined) return;
    void attachedSession
      .respondInteraction(interaction, response)
      .then(() => {
        if (this.#port.isCurrent(lease) && this.#partial()) {void attachedSession.resync().catch(error=>{if(this.#port.isCurrent(lease))this.#port.showTransient('error',`Interaction observation requires recovery: ${compactDiagnostic(error)}`);});}
      })
      .catch((error: unknown) => {
        if (!this.#port.isCurrent(lease)) return;
        if (this.#overlay === overlay) {
          overlay.submissionFailed(interaction);
        }
        this.#port.showTransient("error", `interaction response failed: ${compactDiagnostic(error)}`);
      });
  }

  /** The open human-input surface, if any. */
  get overlay(): HumanInteractionOverlay | undefined { return this.#overlay; }
  /** The presentation-only focused interaction. */
  get focus(): InteractionRef | undefined { return this.#focus; }
  /** Ctrl+G: forget an Esc dismissal so the surface may reopen on the next render. */
  reopen(): boolean {
    if (this.#overlay !== undefined) return false;
    this.#dismissed = undefined;
    return true;
  }
  /** The app's single popup slot closed or was replaced. */
  overlayClosed(): void { this.#overlay = undefined; }
  /** An authoritative replacement re-derives focus from the new projection:
   * no stale dismissed marker may hide an interaction the runtime owns now. */
  reset(): void { this.#focus = undefined; this.#dismissed = undefined; }
  /**
   * The pending interactions this surface presents for one authoritative
   * state: the bounded page viewport and a loaded body, both scoped to the
   * native revision they were read at, with focus reconciled to the result.
   * The viewport never replaces a native snapshot or infers settlement.
   */
  project(state: PresentationState): PresentationState {
    const source = this.#port.session();
    const stamp = this.#stamp();
    if (this.#page && (this.#page.session !== source || this.#page.stamp !== stamp)) {
      this.#page = undefined;
      this.#pageRead += 1;
    }
    if (this.#page) state = { ...state, pendingInteractions: this.#page.items };
    const body = this.#body;
    if (body && body.session === source && body.stamp === stamp) state = { ...state, pendingInteractions: state.pendingInteractions.map(item => sameInteractionRef(item.interaction, body.interaction) ? body.body : item) };
    else if (body) { this.#body = undefined; this.#bodyRead++; }
    // Settling the focused interaction advances focus deterministically, and
    // an empty queue drops it.
    this.#focus = reconcileInteractionFocus(state.pendingInteractions, this.#focus);
    return state;
  }
}
