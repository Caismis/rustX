/* Copyright (c) 2026 DeepSeek. MIT. Rewritten from ui-chat/ChatView.tsx; see PROVENANCE.md. */
import { Component, createRef, type ReactNode } from 'react';
import { IconChevronDownOutline14 } from '../primitives/icons/index.tsx';

interface Anchor { key: string; top: number }
interface ReadingPosition { anchors: Anchor[]; top: number }
interface ViewportProps {
  children: ReactNode;
  overlay?: ReactNode;
  latestLabel?: string;
  historical?: boolean;
  onLatest?: () => void;
  onUserIntent?: () => void;
  latestTurn?: string;
  /** null means observed detached reading with no native owner; undefined is
   * unobserved/follow mode, where the rail may use current native state. */
  onActiveTurn?: (key: string | null | undefined) => void;
}
/** One frame owns every automatic Chat correction. Native user scrolling
 * changes intent synchronously, including while a layout frame is pending. */
export class ChatViewport extends Component<ViewportProps, { detached: boolean }> {
  state = { detached: false };
  private viewport = createRef<HTMLDivElement>();
  private content = createRef<HTMLDivElement>();
  private observer?: ResizeObserver;
  private frame?: number;
  private reading?: ReadingPosition;
  private following = true;
  private writtenTop = 0;
  private mounted = false;
  private intent = 0;
  private navigation?: { intent: number; anchor: string; current: () => boolean };
  private active?: string | null;
  private explicitLatest = false;
  private rows() { return [...(this.content.current?.querySelectorAll<HTMLElement>('[data-chat-anchor-key]') ?? [])].filter(row => !row.closest('[hidden]')); }
  private position(): ReadingPosition | undefined {
    const el = this.viewport.current;
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
  private commitLayout = () => {
    this.frame = undefined;
    const el = this.viewport.current;
    if (!this.mounted || !el) return;
    const floor = Math.max(0, el.scrollHeight - el.clientHeight);
    let desired = el.scrollTop;
    const navigation = this.navigation;
    this.navigation = undefined;
    if (navigation && navigation.intent === this.intent && navigation.current()) {
      const row = this.rows().find(row => row.dataset.chatAnchorKey === navigation.anchor);
      if (row) desired = el.scrollTop + row.getBoundingClientRect().top - el.getBoundingClientRect().top;
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
    const el = this.viewport.current;
    if (!el) return;
    const regions = [...(this.content.current?.querySelectorAll<HTMLElement>('[data-chat-turn-owner], [data-chat-anchor-key]') ?? [])].filter(row => !row.closest('[hidden]'));
    const top = el.getBoundingClientRect().top;
    // Each rendered native-owned region carries identity even when its exact
    // start is outside this finite window. Unowned rows end ownership; an owned
    // final region stays active through its tail. Locate anchors are not owners.
    const owner = regions.reverse().find(row => row.getBoundingClientRect().top <= top)?.dataset.chatTurnOwner;
    const key = this.following && !this.props.historical && this.props.latestTurn ? this.props.latestTurn
      : owner ?? (!this.following || this.props.historical ? null : undefined);
    if (key !== this.active) { this.active = key; this.props.onActiveTurn?.(key); }
  };
  private onScroll = () => {
    const el = this.viewport.current!;
    const floor = Math.max(0, el.scrollHeight - el.clientHeight);
    if (Math.abs(el.scrollTop - Math.min(this.writtenTop, floor)) > 0.5) {
      this.intent++; this.navigation = undefined; this.props.onUserIntent?.();
      this.following = !this.props.historical && floor - el.scrollTop <= 24;
      this.reading = this.following ? undefined : this.position();
      this.setState({ detached: !this.following });
      this.publishActive();
    }
    this.writtenTop = el.scrollTop;
  };
  /** Explicit action changes intent; positioning still belongs to the frame. */
  returnToBottom = () => {
    this.intent++; this.navigation = undefined;
    this.explicitLatest = true;
    this.props.onLatest?.(); this.following = true; this.reading = undefined;
    this.setState({ detached: false }); this.markLayoutDirty();
  };
  beginNavigation = () => {
    const intent = ++this.intent;
    this.navigation = undefined; this.following = false; this.reading = this.position();
    this.setState({ detached: true });
    return { current: () => this.mounted && this.intent === intent,
      commit: (anchor: string, current: () => boolean = () => true) => {
        if (!this.mounted || this.intent !== intent || !current()) return false;
        this.navigation = { intent, anchor, current }; this.markLayoutDirty(); return true;
      } };
  };
  componentDidMount() {
    this.mounted = true;
    this.markLayoutDirty();
    if (typeof ResizeObserver !== 'undefined') {
      this.observer = new ResizeObserver(this.markLayoutDirty);
      this.observer.observe(this.content.current!);
      this.observer.observe(this.viewport.current!);
    }
  }
  getSnapshotBeforeUpdate() {
    // Preserve the original anchor across multiple commits before the frame.
    const position = this.position();
    if (!this.following && this.frame === undefined) this.reading = position;
    return { first: this.rows()[0]?.dataset.chatAnchorKey, position };
  }
  componentDidUpdate(_previous: Readonly<ViewportProps>, _state: unknown, before: { first?: string; position?: ReadingPosition }) {
    const rows = this.rows();
    if (!this.explicitLatest && before.first && rows[0]?.dataset.chatAnchorKey !== before.first && rows.some(row => row.dataset.chatAnchorKey === before.first)) {
      if (this.following) this.reading = before.position;
      this.following = false;
      if (!this.state.detached) this.setState({ detached: true });
    }
    this.markLayoutDirty();
  }
  componentWillUnmount() {
    this.mounted = false;
    this.intent++;
    this.observer?.disconnect();
    if (this.frame !== undefined) cancelAnimationFrame(this.frame);
    this.frame = undefined;
  }
  render() {
    return <div className="chat-reading-surface" style={{ position: 'relative', display: 'flex', flexDirection: 'column', flex: 1, minHeight: 0, minWidth: 0 }}>{this.props.overlay}<div ref={this.viewport} className="conversation-scroll" style={{ overflowAnchor: 'none' }} onScroll={this.onScroll}
      onClickCapture={event => { if ((event.target as HTMLElement).closest('[data-chat-latest]')) this.returnToBottom(); }}>
      <div ref={this.content}>{this.props.children}</div>
    </div>{this.props.latestLabel && (this.state.detached || this.props.historical) && <button type="button" data-chat-latest className="chat-return-latest" aria-label={this.props.latestLabel} title={this.props.latestLabel} onClick={this.returnToBottom}><IconChevronDownOutline14 size={16}/></button>}</div>;
  }
}
