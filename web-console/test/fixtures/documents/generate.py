# Original rustX test fixtures, MIT. Python standard library only.
from pathlib import Path
import zipfile
p=Path(__file__).resolve().parent
ns='http://schemas.openxmlformats.org/'
def package(name,kind,main,parts,types):
    files={'[Content_Types].xml':f'<Types xmlns="{ns}package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/>'+''.join(f'<Override PartName="/{f}" ContentType="{t}"/>' for f,t in types.items())+'</Types>', '_rels/.rels':f'<Relationships xmlns="{ns}package/2006/relationships"><Relationship Id="r1" Type="{ns}officeDocument/2006/relationships/officeDocument" Target="{main}"/></Relationships>',**parts}
    with zipfile.ZipFile(p/name,'w') as z:
        for path,data in files.items():
            info=zipfile.ZipInfo(path,date_time=(2020,1,1,0,0,0));info.compress_type=zipfile.ZIP_DEFLATED
            z.writestr(info,data)
w=ns+'wordprocessingml/2006/main'
package('sample.docx','docx','word/document.xml',{'word/document.xml':f'<w:document xmlns:w="{w}"><w:body><w:p><w:r><w:t>rustX document preview</w:t></w:r></w:p><w:p><w:r><w:t>Original DOCX bytes remain downloadable.</w:t></w:r></w:p><w:sectPr><w:pgSz w:w="12240" w:h="15840"/></w:sectPr></w:body></w:document>'},{'word/document.xml':'application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml'})
a=ns+'drawingml/2006/main'; pres=ns+'presentationml/2006/main'; rel=ns+'officeDocument/2006/relationships'
package('sample.pptx','pptx','ppt/presentation.xml',{'ppt/presentation.xml':f'<p:presentation xmlns:p="{pres}" xmlns:r="{rel}"><p:sldIdLst><p:sldId id="256" r:id="slide1"/></p:sldIdLst><p:sldSz cx="9144000" cy="6858000"/><p:notesSz cx="6858000" cy="9144000"/></p:presentation>', 'ppt/_rels/presentation.xml.rels':f'<Relationships xmlns="{ns}package/2006/relationships"><Relationship Id="slide1" Type="{rel}/slide" Target="slides/slide1.xml"/></Relationships>', 'ppt/slides/slide1.xml':f'<p:sld xmlns:p="{pres}" xmlns:a="{a}"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="500000" y="500000"/><a:ext cx="8000000" cy="2000000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>rustX presentation preview</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>'},{'ppt/presentation.xml':'application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml','ppt/slides/slide1.xml':'application/vnd.openxmlformats-officedocument.presentationml.slide+xml'})
x=ns+'spreadsheetml/2006/main'
package('sample.xlsx','xlsx','xl/workbook.xml',{'xl/workbook.xml':f'<workbook xmlns="{x}" xmlns:r="{rel}"><sheets><sheet name="Values" sheetId="1" r:id="s1"/><sheet name="第二页" sheetId="2" r:id="s2"/></sheets></workbook>','xl/_rels/workbook.xml.rels':f'<Relationships xmlns="{ns}package/2006/relationships"><Relationship Id="s1" Type="{rel}/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="s2" Type="{rel}/worksheet" Target="worksheets/sheet2.xml"/></Relationships>','xl/worksheets/sheet1.xml':f'<worksheet xmlns="{x}"><sheetData><row r="1"><c r="A1"><v>2</v></c><c r="B1"><v>3</v></c><c r="C1"><f>A1+B1</f><v>99</v></c><c r="D1"><f>NOW()</f></c></row></sheetData></worksheet>','xl/worksheets/sheet2.xml':f'<worksheet xmlns="{x}"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Stored text only</t></is></c></row></sheetData></worksheet>'},{'xl/workbook.xml':'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml','xl/worksheets/sheet1.xml':'application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml','xl/worksheets/sheet2.xml':'application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml'})
# Minimal real PDF with correct cross-reference offsets and three textual pages.
objects=['<< /Type /Catalog /Pages 2 0 R >>','<< /Type /Pages /Kids [3 0 R 5 0 R 7 0 R] /Count 3 >>']
for i in range(3):
    stream=f'BT /F1 18 Tf 50 700 Td (rustX page {i+1}: selectable text) Tj ET'
    objects += [f'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 9 0 R >> >> /Contents {4+i*2} 0 R >>',f'<< /Length {len(stream)} >>\nstream\n{stream}\nendstream']
objects+=['<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>']
data=b'%PDF-1.4\n'; offsets=[0]
for i,obj in enumerate(objects,1):
    offsets.append(len(data)); data+=f'{i} 0 obj\n{obj}\nendobj\n'.encode()
xref=len(data); data+=f'xref\n0 {len(offsets)}\n0000000000 65535 f \n'.encode()
for off in offsets[1:]:data+=f'{off:010} 00000 n \n'.encode()
data+=f'trailer\n<< /Size {len(offsets)} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n'.encode();(p/'sample.pdf').write_bytes(data)
(p/'benign.html').write_text('<!doctype html><h1>Local report</h1><p>Only inert local content.</p>')
(p/'hostile.html').write_text('''<!doctype html><meta http-equiv="refresh" content="0;url=https://rustx-preview.invalid/nav"><base href="https://rustx-preview.invalid/"><script>parent.PWNED=1;localStorage.setItem('PWNED','1');window.open('https://rustx-preview.invalid/popup');fetch('https://rustx-preview.invalid/fetch');parent.location='https://rustx-preview.invalid/parent';</script><img src="https://rustx-preview.invalid/image" onerror="parent.PWNED=2"><link rel="stylesheet" href="https://rustx-preview.invalid/style"><style>@import url('https://rustx-preview.invalid/import');body{background:url('https://rustx-preview.invalid/background')}</style><form action="https://rustx-preview.invalid/form" target="_top"><button>Submit</button></form><a href="https://rustx-preview.invalid/link" target="_blank">External</a><iframe src="https://rustx-preview.invalid/frame"></iframe><h1>Hostile fixture</h1>''')
(p/'malformed.pdf').write_bytes(b'%PDF-1.7\nbroken')
