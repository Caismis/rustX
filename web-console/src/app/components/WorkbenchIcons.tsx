/* Copyright (c) 2026 DeepSeek. MIT. Harness guide/tab artwork; see PROVENANCE.md. */
export function CompassGlyph({ size = 16, className }: { size?: number; className?: string }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="none" aria-hidden="true" className={className}>
      <path d="M8 14C11.3137 14 14 11.3137 14 8C14 4.68629 11.3137 2 8 2C4.68629 2 2 4.68629 2 8C2 11.3137 4.68629 14 8 14Z" stroke="currentColor" />
      <path d="M10.6101 5.39014L8.99014 8.99014L5.39014 10.6101L7.01014 7.01014L10.6101 5.39014Z" fill="currentColor" />
    </svg>
  )
}

export function GuideArtworkFiles({ size = 36, className }: { size?: number; className?: string }) {
  return (
    <svg width={size} height={size} className={className} aria-hidden="true" viewBox="0 0 36 36" fill="none" xmlns="http://www.w3.org/2000/svg">
      <path d="M10.7603 27.922H24.6817C26.3441 27.922 27.1753 27.922 27.8102 27.5984C28.3687 27.3139 28.8228 26.8598 29.1074 26.3012C29.4309 25.6663 29.4309 24.8351 29.4309 23.1727V15.4936" stroke="#FFCD78" strokeWidth="1.97886" />
      <path d="M13.1597 8.07812C13.4182 8.07818 13.6727 8.14336 13.8989 8.26855L16.7554 9.84961C16.9817 9.97485 17.2369 10.041 17.4956 10.041H26.106C26.9492 10.0412 27.6323 10.7251 27.6323 11.5684V24.5371C27.6323 25.3805 26.9483 26.0645 26.105 26.0645H8.09619C7.25281 26.0645 6.56884 25.3805 6.56885 24.5371V9.60449C6.56909 8.76133 7.25297 8.07812 8.09619 8.07812H13.1597ZM9.81592 14.5508V16.5293H24.3999V14.5508H9.81592Z" fill="#FFBC4D" />
    </svg>
  )
}
export const PluginArtworkTerminal = ({ size = 36, className }: { size?: number; className?: string }) => (
  <svg width={size} height={size} className={className} viewBox="0 0 36 36" fill="none" xmlns="http://www.w3.org/2000/svg">
    <path d="M10 11L16.606 17.606C16.6841 17.6841 16.6841 17.8107 16.606 17.8888L10 24.4948" stroke="#679EFE" strokeWidth="3.5" />
    <path d="M20.1211 24.4946H26.8685" stroke="#679EFE" strokeWidth="3.5" />
  </svg>
)

/** Terminal glyph for sidebar tab titles. */


/**
 * Render the tab title's terminal prompt in the surrounding text color.
 * @returns a decorative sixteen-pixel line glyph.
 */
export function TerminalIcon() {
  return <svg width="16" height="16" viewBox="0 0 16 16" fill="none" aria-hidden="true">
    <path d="M3 4L7 8L3 12" stroke="currentColor" />
    <path d="M9 12H13" stroke="currentColor" />
  </svg>
}
export function FullscreenGlyph() {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" fill="none" aria-hidden="true">
      <path d="M2.33203 10.4054V13.1681C2.33229 13.444 2.55605 13.6681 2.83203 13.6681H5.49512V14.6681H2.83203C2.00376 14.6681 1.33229 13.9963 1.33203 13.1681V10.4054H2.33203ZM14.6689 13.1681C14.6687 13.996 13.9968 14.6676 13.1689 14.6681H10.4951V13.6681H13.1689C13.4445 13.6676 13.6687 13.4437 13.6689 13.1681V10.4054H14.6689V13.1681ZM13.1689 1.33118C13.9969 1.33163 14.6688 2.00315 14.6689 2.83118V5.4054H13.6689V2.83118C13.6688 2.55544 13.4446 2.33162 13.1689 2.33118H10.4951V1.33118H13.1689ZM5.49512 2.33118H2.83203C2.55598 2.33118 2.33218 2.55516 2.33203 2.83118V5.4054H1.33203V2.83118C1.33218 2.00288 2.00369 1.33118 2.83203 1.33118H5.49512V2.33118Z" fill="currentColor" />
    </svg>
  )
}

/** Restore-from-fullscreen glyph from the shared product artwork. */
export function ExitFullscreenGlyph() {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" fill="none" aria-hidden="true">
      <path d="M9 2.5V6C9 6.26522 9.10536 6.51957 9.29289 6.70711C9.48043 6.89464 9.73478 7 10 7H13.5" stroke="currentColor" />
      <path d="M7 13.5V10C7 9.73478 6.89464 9.48043 6.70711 9.29289C6.51957 9.10536 6.26522 9 6 9H2.5" stroke="currentColor" />
    </svg>
  )
}
