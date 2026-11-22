# Regenerate: python3 make.py . (needs pikepdf, which bundles qpdf; tested with 10.18.2 / qpdf 12.4.2)
# Encrypted fixtures for ferrix-pdf's permission tests, made with qpdf (via pikepdf) so the
# tests don't check lopdf against itself. Usage: make_rights.py <out dir>
import sys
import pikepdf
from pikepdf import Array, Dictionary, Encryption, Name, Permissions, Pdf

out = sys.argv[1]
pdf = Pdf.new()
font = pdf.make_indirect(Dictionary(Type=Name.Font, Subtype=Name.Type1, BaseFont=Name.Helvetica))
for word in ["ALPHA", "BRAVO"]:
    content = pdf.make_stream(f"BT /F1 36 Tf 72 700 Td (PAGE {word}) Tj ET".encode())
    pdf.pages.append(pikepdf.Page(Dictionary(
        Type=Name.Page, MediaBox=Array([0, 0, 612, 792]), Contents=content,
        Resources=Dictionary(Font=Dictionary(F1=font)))))

locked = Permissions(modify_other=False, modify_annotation=False, modify_assembly=False,
                     modify_form=False)
notes_only = Permissions(modify_other=False, modify_annotation=True, modify_assembly=False,
                         modify_form=True)
for name, kw in {"rc4": dict(R=3, aes=False, metadata=False),
                 "aes128": dict(R=4, aes=True),
                 "aes256": dict(R=6, aes=True)}.items():
    for perms_name, perms in {"locked": locked, "notes": notes_only, "open": Permissions()}.items():
        pdf.save(f"{out}/{name}-{perms_name}.pdf",
                 encryption=Encryption(owner="owner", user="", allow=perms, **kw),
                 object_stream_mode=pikepdf.ObjectStreamMode.disable)
    # A user password too, for the CLI's organize check.
    pdf.save(f"{out}/{name}-user.pdf",
             encryption=Encryption(owner="owner", user="user", allow=Permissions(), **kw),
             object_stream_mode=pikepdf.ObjectStreamMode.disable)
