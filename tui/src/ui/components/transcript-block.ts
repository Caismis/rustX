import { Container, Box, Text, Markdown } from "@earendil-works/pi-tui";
import type { TranscriptBlock } from "./transcript.ts";
import { background, markdownTheme } from "../theme.ts";

/**
 * Lays one transcript block out, on its background band when it has one.
 *
 * The caller supplies the available width: a background has to be filled to the edge of the
 * line, and a component that composed one into its own string would paint a
 * ragged block whose colour stopped at its longest line. Pi's `Box` does the
 * filling; everything above only names the band.
 *
 * A banded block owns its horizontal padding through the box, so its inner
 * component takes none — otherwise the padding would be applied twice and the
 * band would sit one column further in than the content it frames.
 */

export function banded(block: TranscriptBlock): Container | Box | Text | Markdown {
  const pad = block.background === undefined ? 1 : 0;
  const content =
    block.kind === "markdown"
      ? new Markdown(
          block.markdown,
          pad,
          0,
          markdownTheme,
          block.defaultTextStyle,
        )
      : new Text(block.text, pad, 0);
  if (block.background === undefined) {
    return content;
  }
  const box = new Box(1, 1, background[block.background]);
  box.addChild(content);
  return box;
}
