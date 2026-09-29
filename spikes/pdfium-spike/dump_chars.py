"""pypdfium2 counterpart of spikes/pdfium-spike (same raw calls, same JSON shape)."""
import ctypes
import json
import sys

import pypdfium2.raw as R


def main():
    path = sys.argv[1]
    R.FPDF_InitLibrary()
    doc = R.FPDF_LoadDocument(path.encode(), None)
    n = R.FPDF_GetPageCount(doc)
    first = int(sys.argv[2]) if len(sys.argv) > 2 else 0
    last = min(int(sys.argv[3]) if len(sys.argv) > 3 else n - 1, n - 1)
    pages = []
    for pi in range(first, last + 1):
        page = R.FPDF_LoadPage(doc, pi)
        tp = R.FPDFText_LoadPage(page)
        chars = []
        for i in range(R.FPDFText_CountChars(tp)):
            ox, oy = ctypes.c_double(), ctypes.c_double()
            R.FPDFText_GetCharOrigin(tp, i, ox, oy)
            l, r, b, t = (ctypes.c_double() for _ in range(4))
            R.FPDFText_GetCharBox(tp, i, l, r, b, t)
            loose = R.FS_RECTF()
            R.FPDFText_GetLooseCharBox(tp, i, loose)
            buf = ctypes.create_string_buffer(256)
            flags = ctypes.c_int()
            ln = R.FPDFText_GetFontInfo(tp, i, buf, 256, flags)
            name = buf.raw[: max(0, min(ln - 1, 255))].decode("utf-8", "replace") if ln > 0 else ""
            chars.append([R.FPDFText_GetUnicode(tp, i), R.FPDFText_IsGenerated(tp, i), [ox.value, oy.value],
                          [l.value, r.value, b.value, t.value],
                          [loose.left, loose.right, loose.bottom, loose.top],
                          R.FPDFText_GetFontSize(tp, i), name, flags.value])
        objs = sum(1 for k in range(R.FPDFPage_CountObjects(page))
                   if R.FPDFPageObj_GetType(R.FPDFPage_GetObject(page, k)) == R.FPDF_PAGEOBJ_TEXT)
        pages.append({"page": pi + 1, "rotation": R.FPDFPage_GetRotation(page), "text_objects_top": objs, "chars": chars})
        R.FPDFText_ClosePage(tp)
        R.FPDF_ClosePage(page)
    R.FPDF_CloseDocument(doc)
    print(json.dumps(pages))


if __name__ == "__main__":
    main()
