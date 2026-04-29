import base64
import io
import os
import tempfile
import threading

from fastapi import FastAPI, HTTPException
from fastapi.middleware.cors import CORSMiddleware
from PIL import Image
from pydantic import BaseModel

app = FastAPI()
app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_methods=["*"],
    allow_headers=["*"],
)

_mpocr = None
_mpocr_lock = threading.Lock()
_cache: dict[str, dict] = {}


def get_mpocr():
    global _mpocr
    if _mpocr is None:
        from mokuro.manga_page_ocr import MangaPageOcr
        _mpocr = MangaPageOcr()
    return _mpocr


class OcrReq(BaseModel):
    image_data: str
    cache_key: str


def run_ocr(pil_img: Image.Image) -> dict:
    with tempfile.NamedTemporaryFile(suffix=".png", delete=False) as tmp:
        path = tmp.name
    try:
        pil_img.save(path)
        with _mpocr_lock:
            return get_mpocr()(path)
    finally:
        try:
            os.unlink(path)
        except OSError:
            pass


@app.post("/ocr")
def ocr(req: OcrReq):
    if req.cache_key in _cache:
        return _cache[req.cache_key]

    try:
        data = req.image_data.split(",", 1)[-1]
        img = Image.open(io.BytesIO(base64.b64decode(data))).convert("RGB")
    except Exception as e:
        raise HTTPException(400, f"bad image: {e}")

    raw = run_ocr(img)

    blocks = []
    for b in raw.get("blocks", []):
        box = b.get("box") or b.get("bbox")
        lines = b.get("lines") or []
        if not box or not lines:
            continue
        blocks.append({
            "box": [float(v) for v in box],
            "vertical": bool(b.get("vertical", False)),
            "font_size": float(b.get("font_size") or 0),
            "lines": [str(l) for l in lines],
        })

    out = {
        "width": int(raw.get("img_width") or img.width),
        "height": int(raw.get("img_height") or img.height),
        "blocks": blocks,
    }
    _cache[req.cache_key] = out
    print(f"[ocr] {req.cache_key[:90]}  blocks={len(blocks)}  size={out['width']}x{out['height']}", flush=True)
    if blocks:
        print(f"       sample: vertical={blocks[0]['vertical']} text={blocks[0]['lines'][:3]}", flush=True)
    return out


@app.get("/health")
def health():
    return {"ok": True, "loaded": _mpocr is not None}
