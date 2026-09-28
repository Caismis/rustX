/* Copyright (c) 2026 DeepSeek. MIT. Rewritten from ui-chat/ChatView.tsx; see PROVENANCE.md. */
import { Component, createRef, type ReactNode } from 'react';

interface Anchor { key: string; top: number }
interface ReadingPosition { anchors: Anchor[]; top: number }
/** One frame owns every automatic Chat correction. Native user scrolling
 * changes intent synchronously, including while a layout frame is pending. */
export class ChatViewport extends Component<{ children: ReactNode }> {
  private viewport = createRef<HTMLDivElement>();
  private content = createRef<HTMLDivElement>();
  private observer?: ResizeObserver;
  private frame?: number;
  private reading?: ReadingPosition;
  private following = true;
  private writtenTop = 0;
  private mounted = false;
  private rows() { return [...(this.content.current?.querySelectorAll<HTMLElement>('[data-chat-anchor-key]') ?? [])]; }
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
    if (this.following) desired = floor;
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
  };
  private onScroll = () => {
    const el = this.viewport.current!;
    const floor = Math.max(0, el.scrollHeight - el.clientHeight);
    if (Math.abs(el.scrollTop - Math.min(this.writtenTop, floor)) > 0.5) {
      this.following = floor - el.scrollTop <= 24;
      this.reading = this.following ? undefined : this.position();
    }
    this.writtenTop = el.scrollTop;
  };
  /** Explicit action changes intent; positioning still belongs to the frame. */
  returnToBottom = () => { this.following = true; this.reading = undefined; this.markLayoutDirty(); };
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
  componentDidUpdate(_previous: Readonly<{ children: ReactNode }>, _state: unknown, before: { first?: string; position?: ReadingPosition }) {
    const rows = this.rows();
    if (before.first && rows[0]?.dataset.chatAnchorKey !== before.first && rows.some(row => row.dataset.chatAnchorKey === before.first)) {
      if (this.following) this.reading = before.position;
      this.following = false;
    }
    this.markLayoutDirty();
  }
  componentWillUnmount() {
    this.mounted = false;
    this.observer?.disconnect();
    if (this.frame !== undefined) cancelAnimationFrame(this.frame);
    this.frame = undefined;
  }
  render() {
    return <div ref={this.viewport} className="conversation-scroll" style={{ overflowAnchor: 'none' }} onScroll={this.onScroll}
      onClickCapture={event => { if ((event.target as HTMLElement).closest('[data-chat-latest]')) this.returnToBottom(); }}>
      <div ref={this.content}>{this.props.children}</div>
    </div>;
  }
}
