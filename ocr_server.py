import base64
import io
import os
import tempfile
import threading
from collections import OrderedDict

from fastapi import FastAPI, HTTPException, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse
from PIL import Image
from pydantic import BaseModel

LOCAL_ORIGINS = {
    "http://127.0.0.1",
    "http://localhost",
    "http://127.0.0.1:7331",
    "http://localhost:7331",
}
CACHE_LIMIT = max(1, int(os.getenv("MOY_CACHE_LIMIT", "256")))

app = FastAPI()
app.add_middleware(
    CORSMiddleware,
    allow_origins=sorted(LOCAL_ORIGINS),
    allow_methods=["GET", "POST"],
    allow_headers=["Content-Type", "X-MOY-Client"],
)

_mpocr = None
_mpocr_lock = threading.Lock()
_cache: OrderedDict[str, dict] = OrderedDict()
_cache_lock = threading.Lock()


def get_mpocr():
    global _mpocr
    if _mpocr is None:
        from mokuro.manga_page_ocr import MangaPageOcr
        _mpocr = MangaPageOcr()
    return _mpocr


class OcrReq(BaseModel):
    image_data: str
    cache_key: str


def normalize_line_coords(raw_coords) -> list[list[list[float]]]:
    out = []
    for coords in raw_coords or []:
        if not isinstance(coords, (list, tuple)):
            continue
        points = []
        for point in coords:
            if not isinstance(point, (list, tuple)) or len(point) < 2:
                continue
            points.append([float(point[0]), float(point[1])])
        if points:
            out.append(points)
    return out


@app.middleware("http")
async def guard_browser_origin(request: Request, call_next):
    origin = request.headers.get("origin")
    if origin and origin not in LOCAL_ORIGINS:
        return JSONResponse({"detail": "origin not allowed"}, status_code=403)
    return await call_next(request)


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
    with _cache_lock:
        cached = _cache.get(req.cache_key)
        if cached is not None:
            _cache.move_to_end(req.cache_key)
            return cached

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
        line_coords = b.get("lines_coords") or b.get("line_coords") or []
        if not box or not lines:
            continue
        blocks.append({
            "box": [float(v) for v in box],
            "vertical": bool(b.get("vertical", False)),
            "font_size": float(b.get("font_size") or 0),
            "lines": [str(l) for l in lines],
            "lines_coords": normalize_line_coords(line_coords),
        })

    out = {
        "width": int(raw.get("img_width") or img.width),
        "height": int(raw.get("img_height") or img.height),
        "blocks": blocks,
    }
    with _cache_lock:
        _cache[req.cache_key] = out
        _cache.move_to_end(req.cache_key)
        while len(_cache) > CACHE_LIMIT:
            _cache.popitem(last=False)
    print(f"[ocr] {req.cache_key[:90]}  blocks={len(blocks)}  size={out['width']}x{out['height']}", flush=True)
    if blocks:
        print(f"       sample: vertical={blocks[0]['vertical']} text={blocks[0]['lines'][:3]}", flush=True)
    return out


@app.get("/health")
def health():
    return {"ok": True, "loaded": _mpocr is not None}
