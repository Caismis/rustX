/* Copyright (c) 2026 DeepSeek. MIT. Rewritten from ui-chat/ChatView.tsx; see PROVENANCE.md. */
import { createPortal } from 'react-dom';
import { Component, createRef, type ReactNode } from 'react';
import { IconChevronDownOutline14 } from '../primitives/icons/index.tsx';

interface Anchor { key: string; top: number }
interface ReadingPosition { anchors: Anchor[]; top: number }
interface ViewportProps {
  children: ReactNode;
  overlay?: ReactNode;
  latestLabel?: string;
  /** Resident hidden chats must not portal floating chrome into a visible sibling. */
  chromeVisible?: boolean;
  historical?: boolean;
  onLatest?: () => void;
  latestTurn?: string;
  /** null means observed detached reading with no native owner; undefined is
   * unobserved/follow mode, where the rail may use current native state. */
  onActiveTurn?: (key: string | null | undefined) => void;
}
/** One frame owns every automatic Chat correction. Native user scrolling
 * changes intent synchronously, including while a layout frame is pending. */
export class ChatViewport extends Component<ViewportProps, { detached: boolean; chromeHost?: HTMLElement }> {
  state: { detached: boolean; chromeHost?: HTMLElement } = { detached: false };
  private scroller = () => this.viewport.current?.closest<HTMLElement>('[data-conversation-scroll]') ?? this.viewport.current;
  private viewport = createRef<HTMLDivElement>();
  private content = createRef<HTMLDivElement>();
  private observer?: ResizeObserver;
  private frame?: number;
  private reading?: ReadingPosition;
  private following = true;
  private writtenTop = 0;
  private mounted = false;
  private intent = 0;
  private retireGesture?: () => void;
  private retireNavigation = () => {
    const retire = this.retireGesture; this.retireGesture = undefined;
    this.intent++; this.navigation = undefined; this.landed = undefined; retire?.();
  };
  private navigation?: { intent: number; anchor: string; current: () => boolean };
  private landed?: { anchor: string; current: () => boolean };
  private active?: string | null;
  private explicitLatest = false;
  private rows() { return [...(this.content.current?.querySelectorAll<HTMLElement>('[data-chat-anchor-key]') ?? [])].filter(row => !row.closest('[hidden]')); }
  private position(): ReadingPosition | undefined {
    const el = this.scroller();
    if (!el) return;
    const top = el.getBoundingClientRect().top, rows = this.rows();
    const index = Math.max(0, rows.findIndex(row => row.getBoundingClientRect().bottom > top));
    // A vanished row falls back to the next, then preceding row, then a clamped
    // absolute reading offset. No missing-anchor fallback enters follow mode.
    return { top: el.scrollTop, anchors: [rows[index], rows[index + 1], rows[index - 1]].filter(Boolean).map(row => ({ key: row.dataset.chatAnchorKey!, top: row.getBoundingClientRect().top - top })) };
  }
  private markLayoutDirty = () => {
    if (!this.mounted || this.frame !== undefined) return;
    this.frame = requestAnimationFrame(this.commitLayout);
  };
  private onComposerBeforeInput = (event: Event) => {
    if ((event.target as Element).closest('[data-composer-seat]')) this.onScroll();
  };
  private onComposerInput = (event: Event) => {
    if (!(event.target as Element).closest('[data-composer-seat]')) return;
    // Editing can move the scrollport as the browser reveals the textarea's
    // caret (especially replacing a multiline selection). Acknowledge that
    // layout movement without turning it into reader intent. The frame then
    // follows the tail or restores the existing reading anchor as usual.
    this.writtenTop = this.scroller()!.scrollTop;
    this.markLayoutDirty();
  };
  private commitLayout = () => {
    this.frame = undefined;
    const el = this.scroller();
    if (!this.mounted || !el || el.closest('[hidden]')) return;
    // Native scrolling can precede its scroll event. Adopt reader movement
    // before a queued layout correction gets a chance to overwrite it.
    this.onScroll();
    const floor = Math.max(0, el.scrollHeight - el.clientHeight);
    el.querySelector('[data-composer-seat]')?.toggleAttribute('data-sticky-overflow', floor > 0);
    let desired = el.scrollTop;
    const navigation = this.navigation;
    this.navigation = undefined;
    if (navigation && navigation.intent === this.intent && navigation.current()) {
      this.retireGesture = undefined;
      const row = this.rows().find(row => row.dataset.chatAnchorKey === navigation.anchor);
      if (row) {
        desired = el.scrollTop + row.getBoundingClientRect().top - el.getBoundingClientRect().top;
        this.landed = navigation;
      }
    } else if (this.following) desired = floor;
    else if (this.reading) {
      const rows = this.rows();
      const anchor = this.reading.anchors.find(anchor => rows.some(row => row.dataset.chatAnchorKey === anchor.key));
      const row = anchor && rows.find(row => row.dataset.chatAnchorKey === anchor.key);
      desired = row && anchor ? el.scrollTop + row.getBoundingClientRect().top - el.getBoundingClientRect().top - anchor.top : this.reading.top;
    }
    desired = Math.max(0, Math.min(floor, desired));
    // The sole automatic scroll-position assignment in Chat.
    if (Math.abs(el.scrollTop - desired) > 0.5) el.scrollTop = desired;
    this.writtenTop = el.scrollTop;
    this.reading = this.following ? undefined : this.position();
    this.explicitLatest = false;
    this.publishActive();
  };
  private publishActive = () => {
    const el = this.scroller();
    if (!el) return;
    const regions = [...(this.content.current?.querySelectorAll<HTMLElement>('[data-chat-turn-owner]') ?? [])].filter(row => !row.closest('[hidden]'));
    // Match Harness: read inside the content, below the scrollport padding.
    const line = el.getBoundingClientRect().top + Math.min(96, el.clientHeight * 0.2);
    // Like Harness, gaps and ordinary message anchors keep the preceding
    // reading turn; before the first region, select the first loaded turn.
    // This is reading proximity, not ownership of those unowned messages:
    // only native-owned regions supply identities, never exact locate anchors.
    // Tail following also applies after an Attempt settles. A short final
    // reply need not reach the reading line to own the bottom of the page.
    const first = regions[0];
    const region = this.following ? regions.at(-1)
      : regions.reverse().find(row => row.getBoundingClientRect().top <= line) ?? first;
    const owner = region?.dataset.chatTurnOwner;
    // A successful explicit jump owns selection until the reader moves.
    // The final prompt may be clamped by the scroll limit, or its owned
    // response may begin below the reading line. Neither selects its predecessor.
    const landed = this.landed;
    const located = landed?.current() && regions.some(row => row.dataset.chatTurnOwner === landed.anchor)
      ? landed.anchor : undefined;
    if (!located) this.landed = undefined;
    const key = located ?? (this.following && this.props.latestTurn ? this.props.latestTurn
      : owner ?? (this.following ? undefined : null));
    if (key !== this.active) { this.active = key; this.props.onActiveTurn?.(key); }
  };
  private onScroll = () => {
    const el = this.scroller()!;
    // Resident conversations disappear from layout while a sibling is open.
    // Their zero geometry is not a scroll gesture or a new reading position.
    if (el.closest('[hidden]')) return;
    const floor = Math.max(0, el.scrollHeight - el.clientHeight);
    const previous = Math.min(this.writtenTop, floor);
    if (el.scrollTop < previous || Math.abs(el.scrollTop - previous) > 0.5) {
      this.retireNavigation();
      // A small upward gesture is reading intent even inside the bottom
      // tolerance. Only a downward arrival may re-enable tail following.
      this.following = el.scrollTop > this.writtenTop && floor - el.scrollTop <= 24;
      this.reading = this.following ? undefined : this.position();
      if (this.state.detached === this.following) this.setState({ detached: !this.following });
      this.publishActive();
    }
    this.writtenTop = el.scrollTop;
  };
  /** Explicit action changes intent; positioning still belongs to the frame. */
  returnToBottom = () => {
    this.props.onLatest?.();
    this.writtenTop = this.scroller()?.scrollTop ?? this.writtenTop;
    this.retireNavigation();
    this.explicitLatest = true;
    this.following = true; this.reading = undefined;
    this.setState({ detached: false }); this.markLayoutDirty();
  };
  beginNavigation = (retired?: () => void) => {
    this.retireNavigation();
    this.retireGesture = retired;
    this.writtenTop = this.scroller()?.scrollTop ?? this.writtenTop;
    const intent = this.intent;
    this.navigation = undefined; this.following = false; this.reading = this.position();
    this.setState({ detached: true });
    const current = () => { if (this.mounted) this.onScroll(); return this.mounted && this.intent === intent; };
    return { current,
      commit: (anchor: string, current: () => boolean = () => true) => {
        if (!this.mounted || this.intent !== intent || !current()) return false;
        this.navigation = { intent, anchor, current }; this.markLayoutDirty(); return true;
      } };
  };
  componentDidMount() {
    this.mounted = true;
    const scroller = this.scroller()!;
    // A resident session scroller can retain the previous view's position.
    // Its initial offset is layout state, not a new reader gesture.
    this.writtenTop = scroller.scrollTop;
    scroller.addEventListener('scroll', this.onScroll);
    scroller.addEventListener('beforeinput', this.onComposerBeforeInput, true);
    scroller.addEventListener('input', this.onComposerInput);
    if (scroller !== this.viewport.current) this.setState({ chromeHost: scroller.parentElement! });
    this.markLayoutDirty();
    if (typeof ResizeObserver !== 'undefined') {
      this.observer = new ResizeObserver(this.markLayoutDirty);
      this.observer.observe(this.content.current!);
      this.observer.observe(scroller);
      const composer = scroller.querySelector('[data-composer-seat]');
      if (composer) this.observer.observe(composer);
    }
  }
  getSnapshotBeforeUpdate() {
    if (this.scroller()?.closest('[hidden]')) return {};
    // Preserve the original anchor across multiple commits before the frame.
    const position = this.position();
    if (!this.following && this.frame === undefined) this.reading = position;
    return { first: this.rows()[0]?.dataset.chatAnchorKey, position };
  }
  componentDidUpdate(_previous: Readonly<ViewportProps>, _state: unknown, before: { first?: string; position?: ReadingPosition }) {
    if (this.scroller()?.closest('[hidden]')) return;
    const rows = this.rows();
    if (!this.explicitLatest && before.first && rows[0]?.dataset.chatAnchorKey !== before.first && rows.some(row => row.dataset.chatAnchorKey === before.first)) {
      if (this.following) this.reading = before.position;
      this.following = false;
      if (!this.state.detached) this.setState({ detached: true });
    }
    this.markLayoutDirty();
  }
  componentWillUnmount() {
    this.scroller()?.querySelector('[data-composer-seat]')?.removeAttribute('data-sticky-overflow');
    this.scroller()?.removeEventListener('scroll', this.onScroll);
    this.scroller()?.removeEventListener('beforeinput', this.onComposerBeforeInput, true);
    this.scroller()?.removeEventListener('input', this.onComposerInput);
    this.mounted = false;
    this.retireNavigation();
    this.observer?.disconnect();
    if (this.frame !== undefined) cancelAnimationFrame(this.frame);
    this.frame = undefined;
  }
  render() {
    const chrome = this.props.chromeVisible === false ? null : <>{this.props.overlay}{this.props.latestLabel && (this.state.detached || this.props.historical) && <button type="button" data-chat-latest className="chat-return-latest" aria-label={this.props.latestLabel} title={this.props.latestLabel} onClick={this.returnToBottom}><IconChevronDownOutline14 size={16}/></button>}</>;
    return <div className="chat-reading-surface">
      <div ref={this.viewport} className="conversation-scroll" style={{ overflowAnchor: 'none' }}>
        <div ref={this.content}>{this.props.children}</div>
      </div>
      {this.state.chromeHost ? createPortal(chrome, this.state.chromeHost) : chrome}
    </div>;
  }
}
