//! Deterministic synthetic fixture generation for native Read document
//! regressions (test-only).
//!
//! Every fixture is produced in-memory by this module from known constant
//! content: hand-written minimal PDF syntax and minimal OOXML (zip)
//! documents built with stored (uncompressed) entries. The committed
//! corpus under `tests/fixtures/read/documents/` is a byte-for-byte copy of
//! the [`corpus`] output, proven by
//! [`super::tests::committed_fixture_corpus_matches_the_in_repo_generator`],
//! so every binary fixture in the repository is reviewable through this
//! source and reproducible with:
//!
//! ```text
//! cargo test regenerate_committed_fixture_corpus -- --ignored
//! ```

use std::fmt::Write as _;

/// Narrows a fixture counter whose bound is tiny by construction. The
/// explicit call documents the invariant instead of scattering `as` casts.
fn narrow<T: TryFrom<usize>>(value: usize) -> T {
    T::try_from(value)
        .ok()
        .expect("fixture value fits the target integer type")
}

/// Collects rendered XML fragments into one owned string.
fn xml_fragments(parts: impl IntoIterator<Item = String>) -> String {
    parts.into_iter().collect()
}

/// The relative corpus layout of the committed fixture directory.
pub(super) fn corpus() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        (
            "pdf/small-text.pdf",
            pdf(&[&[
                "The rustX document projection.",
                "Hello from a text-layer PDF.",
            ]]),
        ),
        (
            "pdf/multi-page-text.pdf",
            pdf(&[
                &["Page one heading.", "First page body text."],
                &["Page two heading.", "Second page body text."],
            ]),
        ),
        ("pdf/table-heavy.pdf", table_pdf()),
        ("pdf/scanned-no-text.pdf", pdf(&[])),
        ("docx/small.docx", docx()),
        ("docx/larger.docx", docx_with_paragraphs(8)),
        ("xlsx/small.xlsx", xlsx()),
        ("xlsx/larger-sheet.xlsx", xlsx_with_rows(40)),
        ("pptx/small.pptx", pptx_with_slides(2)),
        ("pptx/larger-deck.pptx", pptx_with_slides(4)),
    ]
}

// ---------------------------------------------------------------------------
// Minimal PDF writer
// ---------------------------------------------------------------------------

/// Escapes PDF literal-string bytes.
fn pdf_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '(' => out.push_str("\\("),
            ')' => out.push_str("\\)"),
            '\\' => out.push_str("\\\\"),
            _ => out.push(c),
        }
    }
    out
}

/// One text run at an absolute position.
struct TextRun {
    x: u32,
    y: u32,
    text: String,
}

/// Builds a minimal deterministic PDF: one Helvetica page per `pages`
/// entry, every run positioned absolutely. An empty `pages` slice produces
/// a valid PDF whose pages carry no text operators at all (the
/// scanned/no-text fixture).
fn pdf_runs(pages: &[&[TextRun]]) -> Vec<u8> {
    let mut objects: Vec<String> = Vec::new();
    // 1: catalog, 2: page tree, 3: font, then per page: page object +
    // content stream.
    let page_ids: Vec<usize> = (0..pages.len()).map(|i| 4 + 2 * i).collect();
    objects.push("<< /Type /Catalog /Pages 2 0 R >>".to_owned());
    let kids: Vec<String> = page_ids.iter().map(|id| format!("{id} 0 R")).collect();
    objects.push(format!(
        "<< /Type /Pages /Kids [{}] /Count {} >>",
        kids.join(" "),
        pages.len()
    ));
    objects.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned());
    for (index, runs) in pages.iter().enumerate() {
        let page_id = page_ids[index];
        let content_id = page_id + 1;
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {content_id} 0 R >>"
        ));
        let mut content = String::new();
        let runs: &[TextRun] = runs;
        for run in runs {
            writeln!(
                content,
                "BT /F1 12 Tf {} {} Td ({}) Tj ET",
                run.x,
                run.y,
                pdf_string(&run.text)
            )
            .expect("fixture content write");
        }
        objects.push(format!(
            "<< /Length {} >>\nstream\n{content}endstream",
            content.len()
        ));
    }

    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-1.4\n");
    let mut offsets = Vec::with_capacity(objects.len());
    for (index, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref_start = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_start}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// Renders each `lines` entry as one text line down the page.
pub(super) fn pdf(pages: &[&[&str]]) -> Vec<u8> {
    let pages: Vec<Vec<TextRun>> = pages
        .iter()
        .map(|lines| {
            lines
                .iter()
                .enumerate()
                .map(|(index, line)| TextRun {
                    x: 72,
                    y: 720 - 16 * narrow::<u32>(index),
                    text: (*line).to_owned(),
                })
                .collect()
        })
        .collect();
    let page_refs: Vec<&[TextRun]> = pages.iter().map(Vec::as_slice).collect();
    pdf_runs(&page_refs)
}

/// A three-row two-column table layout: the columns are separate text runs
/// at fixed x positions, the way a real table-heavy PDF positions cells.
fn table_pdf() -> Vec<u8> {
    let rows = [("Item", "Price"), ("Widget", "9.99"), ("Gadget", "24.50")];
    let runs: Vec<TextRun> = rows
        .iter()
        .enumerate()
        .flat_map(|(index, (left, right))| {
            let y = 720 - 16 * narrow::<u32>(index);
            [
                TextRun {
                    x: 72,
                    y,
                    text: (*left).to_owned(),
                },
                TextRun {
                    x: 300,
                    y,
                    text: (*right).to_owned(),
                },
            ]
        })
        .collect();
    pdf_runs(&[&runs])
}

// ---------------------------------------------------------------------------
// Minimal OOXML (zip) writer
// ---------------------------------------------------------------------------

fn crc32(bytes: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (index, entry) in table.iter_mut().enumerate() {
        let mut crc = narrow::<u32>(index);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                0xEDB8_8320 ^ (crc >> 1)
            } else {
                crc >> 1
            };
        }
        *entry = crc;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc = table[((crc ^ u32::from(byte)) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

/// Serializes `entries` as a deterministic zip archive with stored
/// (uncompressed) entries and fixed timestamps.
pub(super) fn zip(entries: &[(&str, String)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, content) in entries {
        let data = content.as_bytes();
        let crc = crc32(data);
        let offset = narrow::<u32>(out.len());
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&0u16.to_le_bytes()); // stored
        out.extend_from_slice(&0u16.to_le_bytes()); // time
        out.extend_from_slice(&0x2821u16.to_le_bytes()); // date (1980-01-01)
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&narrow::<u32>(data.len()).to_le_bytes());
        out.extend_from_slice(&narrow::<u32>(data.len()).to_le_bytes());
        out.extend_from_slice(&narrow::<u16>(name.len()).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);

        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central.extend_from_slice(&0u16.to_le_bytes()); // flags
        central.extend_from_slice(&0u16.to_le_bytes()); // stored
        central.extend_from_slice(&0u16.to_le_bytes()); // time
        central.extend_from_slice(&0x2821u16.to_le_bytes()); // date
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&narrow::<u32>(data.len()).to_le_bytes());
        central.extend_from_slice(&narrow::<u32>(data.len()).to_le_bytes());
        central.extend_from_slice(&narrow::<u16>(name.len()).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let central_offset = narrow::<u32>(out.len());
    let central_size = narrow::<u32>(central.len());
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // disk
    out.extend_from_slice(&0u16.to_le_bytes()); // cd disk
    out.extend_from_slice(&narrow::<u16>(entries.len()).to_le_bytes());
    out.extend_from_slice(&narrow::<u16>(entries.len()).to_le_bytes());
    out.extend_from_slice(&central_size.to_le_bytes());
    out.extend_from_slice(&central_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment
    out
}

/// Wraps DOCX body XML into a minimal complete package.
fn docx_package(body: &str) -> Vec<u8> {
    let content_types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/></Types>"#;
    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
    let document_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/></Relationships>"#;
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}</w:body></w:document>"#
    );
    let numbering = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/><w:lvlText w:val="*"/></w:lvl></w:abstractNum></w:numbering>"#;
    zip(&[
        ("[Content_Types].xml", content_types.to_owned()),
        ("_rels/.rels", rels.to_owned()),
        ("word/_rels/document.xml.rels", document_rels.to_owned()),
        ("word/document.xml", document),
        ("word/numbering.xml", numbering.to_owned()),
    ])
}

fn docx_paragraph(text: &str) -> String {
    format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
}

fn docx_heading(text: &str) -> String {
    format!("<w:p><w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>")
}

fn docx_bullet(text: &str) -> String {
    format!(
        "<w:p><w:pPr><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"1\"/></w:numPr></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>"
    )
}

fn docx_table(rows: &[&[&str]]) -> String {
    let rows = xml_fragments(rows.iter().map(|row| {
        let cells = xml_fragments(
            row.iter()
                .map(|cell| format!("<w:tc><w:p><w:r><w:t>{cell}</w:t></w:r></w:p></w:tc>")),
        );
        format!("<w:tr>{cells}</w:tr>")
    }));
    format!("<w:tbl>{rows}</w:tbl>")
}

/// A small complete DOCX: heading, paragraphs, bullets, and a table.
pub(super) fn docx() -> Vec<u8> {
    let body = [
        docx_heading("Quarterly Report"),
        docx_paragraph("Revenue grew in every region this quarter."),
        docx_paragraph("Key highlights:"),
        docx_bullet("North America led growth"),
        docx_bullet("Europe followed closely"),
        docx_table(&[&["Region", "Revenue"], &["North", "120"], &["South", "95"]]),
    ]
    .join("");
    docx_package(&body)
}

/// A DOCX with one heading and `count` body paragraphs.
pub(super) fn docx_with_paragraphs(count: usize) -> Vec<u8> {
    let mut body = docx_heading("Extended Notes");
    for index in 0..count {
        body.push_str(&docx_paragraph(&format!(
            "Note {index:04}: the projector preserves ordinary paragraphs exactly."
        )));
    }
    docx_package(&body)
}

/// Wraps XLSX sheet XML parts into a minimal complete workbook package.
/// `sheets` holds `(name, sheetData XML)` pairs.
fn xlsx_package(sheets: &[(&str, &str)]) -> Vec<u8> {
    let content_types = {
        let mut overrides = String::new();
        for (index, _) in sheets.iter().enumerate() {
            let _ = write!(
                overrides,
                "<Override PartName=\"/xl/worksheets/sheet{}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>",
                index + 1
            );
        }
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>{overrides}</Types>"
        )
    };
    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;
    let sheet_entries: Vec<String> = (0..sheets.len())
        .map(|index| {
            format!(
                "<sheet name=\"{}\" sheetId=\"{}\" r:id=\"rId{}\"/>",
                sheets[index].0,
                index + 1,
                index + 1
            )
        })
        .collect();
    let workbook = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets>{}</sheets></workbook>",
        sheet_entries.join("")
    );
    let workbook_rels = {
        let mut relationships = String::new();
        for index in 0..sheets.len() {
            let _ = write!(
                relationships,
                "<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet{}.xml\"/>",
                index + 1,
                index + 1
            );
        }
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{relationships}</Relationships>"
        )
    };
    let mut entries: Vec<(&str, String)> = vec![
        ("[Content_Types].xml", content_types),
        ("_rels/.rels", rels.to_owned()),
        ("xl/workbook.xml", workbook),
        ("xl/_rels/workbook.xml.rels", workbook_rels),
    ];
    for (index, (_, sheet_data)) in sheets.iter().enumerate() {
        entries.push((
            match index {
                0 => "xl/worksheets/sheet1.xml",
                1 => "xl/worksheets/sheet2.xml",
                _ => unreachable!("fixture workbooks have at most two sheets"),
            },
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>{sheet_data}</sheetData></worksheet>"
            ),
        ));
    }
    let borrowed: Vec<(&str, String)> = entries;
    zip(&borrowed)
}

fn xlsx_row(row: u32, cells: &[&str]) -> String {
    let cells = xml_fragments(cells.iter().enumerate().map(|(index, cell)| {
        let column = (b'A' + narrow::<u8>(index)) as char;
        format!("<c r=\"{column}{row}\" t=\"inlineStr\"><is><t>{cell}</t></is></c>")
    }));
    format!("<row r=\"{row}\">{cells}</row>")
}

/// A small two-sheet workbook with inline-string cells.
pub(super) fn xlsx() -> Vec<u8> {
    let summary = [
        xlsx_row(1, &["Metric", "Value"]),
        xlsx_row(2, &["Total revenue", "215"]),
        xlsx_row(3, &["Active users", "1820"]),
    ]
    .join("");
    let data = [
        xlsx_row(1, &["Region", "Q1"]),
        xlsx_row(2, &["North", "120"]),
    ]
    .join("");
    xlsx_package(&[("Summary", &summary), ("Data", &data)])
}

/// A single-sheet workbook with `count` numbered rows, used to prove line
/// bounds on projected table text.
pub(super) fn xlsx_with_rows(count: u32) -> Vec<u8> {
    let mut rows = xlsx_row(1, &["Row", "Note"]);
    for index in 1..=count {
        rows.push_str(&xlsx_row(index + 1, &[&format!("row-{index:06}"), "x"]));
    }
    xlsx_package(&[("Rows", &rows)])
}

/// Wraps PPTX slide bodies into a minimal complete deck. Each slide is
/// `(title, body lines)`.
fn pptx_package(slides: &[(&str, &[&str])]) -> Vec<u8> {
    let content_types = {
        let mut overrides = String::new();
        for index in 0..slides.len() {
            let _ = write!(
                overrides,
                "<Override PartName=\"/ppt/slides/slide{}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slide+xml\"/>",
                index + 1
            );
        }
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/ppt/presentation.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml\"/>{overrides}</Types>"
        )
    };
    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#;
    let slide_ids: String = xml_fragments((0..slides.len()).map(|index| {
        format!(
            "<p:sldId id=\"{}\" r:id=\"rId{}\"/>",
            256 + narrow::<u32>(index),
            index + 1
        )
    }));
    let presentation = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<p:presentation xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><p:sldIdLst>{slide_ids}</p:sldIdLst></p:presentation>"
    );
    let presentation_rels = {
        let mut relationships = String::new();
        for index in 0..slides.len() {
            let _ = write!(
                relationships,
                "<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide\" Target=\"slides/slide{}.xml\"/>",
                index + 1,
                index + 1
            );
        }
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{relationships}</Relationships>"
        )
    };
    let mut entries: Vec<(&str, String)> = vec![
        ("[Content_Types].xml", content_types),
        ("_rels/.rels", rels.to_owned()),
        ("ppt/presentation.xml", presentation),
        ("ppt/_rels/presentation.xml.rels", presentation_rels),
    ];
    for (index, (title, lines)) in slides.iter().enumerate() {
        let slide_id = index + 1;
        let paragraphs = xml_fragments(
            lines
                .iter()
                .map(|line| format!("<a:p><a:r><a:t>{line}</a:t></a:r></a:p>")),
        );
        let slide = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<p:sld xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id=\"2\" name=\"Title\"/><p:cNvSpPr><p:nvPr><p:ph type=\"title\"/></p:nvPr></p:cNvSpPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>{title}</a:t></a:r></a:p></p:txBody></p:sp><p:sp><p:nvSpPr><p:cNvPr id=\"3\" name=\"Content\"/><p:cNvSpPr><p:nvPr><p:ph type=\"body\"/></p:nvPr></p:cNvSpPr></p:nvSpPr><p:txBody>{paragraphs}</p:txBody></p:sp></p:spTree></p:cSld></p:sld>"
        );
        entries.push((
            match slide_id {
                1 => "ppt/slides/slide1.xml",
                2 => "ppt/slides/slide2.xml",
                3 => "ppt/slides/slide3.xml",
                4 => "ppt/slides/slide4.xml",
                _ => unreachable!("fixture decks have at most four slides"),
            },
            slide,
        ));
    }
    zip(&entries)
}

/// A small deck with titled slides and body text.
pub(super) fn pptx_with_slides(count: usize) -> Vec<u8> {
    let all: [(&str, &[&str]); 4] = [
        (
            "Project Kickoff",
            &[
                "Milestone one lands in March",
                "Milestone two lands in June",
            ],
        ),
        ("Budget Review", &["Infrastructure stays flat"]),
        ("Staffing", &["Two backend hires approved"]),
        ("Timeline", &["Beta opens to customers in Q3"]),
    ];
    pptx_package(&all[..count])
}
