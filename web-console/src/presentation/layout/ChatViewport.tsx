/* Copyright (c) 2026 DeepSeek. MIT. Rewritten from ui-chat/ChatView.tsx; see PROVENANCE.md. */
import { Component, createRef, type ReactNode } from 'react';

interface Anchor { key: string; top: number }
/** Stable row measurements are presentation only; the caller supplies identities. */
export class ChatViewport extends Component<{ children: ReactNode }> {
  private viewport = createRef<HTMLDivElement>();
  private content = createRef<HTMLDivElement>();
  private observer?: ResizeObserver;
  private anchor?: Anchor;
  private following = true;
  private writtenTop = 0;
  private rows() { return [...(this.content.current?.querySelectorAll<HTMLElement>('[data-chat-anchor-key]') ?? [])]; }
  private capture = () => {
    const el = this.viewport.current;
    if (!el || this.following) { this.anchor = undefined; return; }
    const top = el.getBoundingClientRect().top;
    const rows = this.rows();
    const row = rows.find(row => row.getBoundingClientRect().bottom > top) ?? rows.at(-1);
    if (row) this.anchor = { key: row.dataset.chatAnchorKey!, top: row.getBoundingClientRect().top - top };
  };
  private restore = () => {
    const el = this.viewport.current;
    if (!el) return;
    if (this.following) el.scrollTop = Math.max(0, el.scrollHeight - el.clientHeight);
    else if (this.anchor) {
      const row = this.rows().find(row => row.dataset.chatAnchorKey === this.anchor!.key);
      if (row) el.scrollTop += row.getBoundingClientRect().top - el.getBoundingClientRect().top - this.anchor.top;
    }
    this.writtenTop = el.scrollTop;
    this.capture();
  };
  private onScroll = () => {
    const el = this.viewport.current!;
    const floor = Math.max(0, el.scrollHeight - el.clientHeight);
    if (Math.abs(el.scrollTop - Math.min(this.writtenTop, floor)) > 0.5) {
      this.following = floor - el.scrollTop <= 24;
      this.capture();
    }
    this.writtenTop = el.scrollTop;
  };
  componentDidMount() {
    this.restore();
    if (typeof ResizeObserver !== 'undefined') {
      this.observer = new ResizeObserver(this.restore);
      this.observer.observe(this.content.current!);
      this.observer.observe(this.viewport.current!);
    }
  }
  getSnapshotBeforeUpdate() {
    this.capture();
    const el = this.viewport.current!;
    const rows = this.rows();
    const row = rows.find(row => row.getBoundingClientRect().bottom > el.getBoundingClientRect().top) ?? rows.at(-1);
    return { first: rows[0]?.dataset.chatAnchorKey, anchor: row ? { key: row.dataset.chatAnchorKey!, top: row.getBoundingClientRect().top - el.getBoundingClientRect().top } : undefined };
  }
  componentDidUpdate(_previous: Readonly<{ children: ReactNode }>, _state: unknown, before: { first?: string; anchor?: Anchor }) {
    const rows = this.rows();
    if (before.first && rows[0]?.dataset.chatAnchorKey !== before.first && rows.some(row => row.dataset.chatAnchorKey === before.first)) {
      // Loading earlier explicitly enters history reading even if the old
      // short window fitted entirely in the viewport.
      this.following = false;
      this.anchor = before.anchor;
    }
    this.restore();
  }
  componentWillUnmount() { this.observer?.disconnect(); }
  render() {
    return <div ref={this.viewport} className="conversation-scroll" style={{ overflowAnchor: 'none' }} onScroll={this.onScroll}>
      <div ref={this.content}>{this.props.children}</div>
    </div>;
  }
}
