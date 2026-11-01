"""Сравнение OCR-движков на чеках. Всё работает локально, сеть не используется.

    python bench.py IMAGES_DIR --truth TRUTH_DIR --out OUT_DIR [--engines tess_fast,tess_best,ppocr,ocrs]

IMAGES_DIR: фото чеков. TRUTH_DIR: <имя>.json с эталоном (формат в README).
Без эталона считаются только скорость и доля чеков, где найден итог.

Приватность: распознанный текст пишется только в OUT_DIR/<движок>/*.txt.
В stdout и report.json нет текста чеков, только флаги и агрегаты, поэтому
отчёт можно показывать. Для реальных чеков держите OUT_DIR внутри samples/.
Парсер здесь прототипный и нужен только для оценки по целям из ТЗ.
"""
import argparse
import difflib
import hashlib
import json
import re
import statistics
import subprocess
import sys
import time
import unicodedata
from pathlib import Path

import numpy as np
from PIL import Image, ImageFilter, ImageOps
from pillow_heif import register_heif_opener

register_heif_opener()  # фото с iPhone

ROOT = Path(__file__).parent
CACHE = ROOT / ".cache"
PL = set("ąćęłńóśźżĄĆĘŁŃÓŚŹŻ")
IMG_EXT = {".jpg", ".jpeg", ".png", ".heic", ".webp"}


# ---------- предобработка ----------
def deskew(g):
    small = g.copy()
    small.thumbnail((700, 700))
    # Ищем наклон по штрихам текста, а не по тёмному фону вокруг чека
    bg = np.asarray(small.filter(ImageFilter.GaussianBlur(12)), dtype=np.float32) + 1
    ink = np.asarray(small, dtype=np.float32) / bg < 0.82
    pad = int(min(small.size) * 0.08)
    ink[:pad], ink[-pad:], ink[:, :pad], ink[:, -pad:] = False, False, False, False
    inkimg = Image.fromarray((ink * 255).astype(np.uint8))
    best, best_score = 0.0, -1.0
    for ang in np.arange(-6, 6.01, 0.25):
        r = np.asarray(inkimg.rotate(ang, resample=Image.BILINEAR), dtype=np.float32)
        score = float(np.var(r.sum(axis=1)))
        if score > best_score:
            best, best_score = float(ang), score
    return g.rotate(best, resample=Image.BICUBIC, fillcolor=255) if best else g


def preprocess(path, binarize):
    g = ImageOps.exif_transpose(Image.open(path)).convert("L")
    g.thumbnail((2400, 2400))
    # Выравнивание нужно всем: иначе колонка цен съезжает на соседнюю строку
    g = deskew(g)
    if not binarize:
        return g
    if g.width < 1000:
        g = g.resize((g.width * 2, g.height * 2), Image.LANCZOS)
    g = g.filter(ImageFilter.MedianFilter(3))
    a = np.asarray(g, dtype=np.float32)
    # Выравнивание света делением на фон, затем растяжка контраста.
    # Полутона сохраняем: жёсткий порог съедает штрихи бледной термопечати.
    bg = np.asarray(g.filter(ImageFilter.GaussianBlur(35)), dtype=np.float32) + 1
    r = a / bg
    lo, hi = np.percentile(r, 1.5), np.percentile(r, 60)
    out = (np.clip((r - lo) / max(hi - lo, 1e-3), 0, 1) * 255).astype(np.uint8)
    return Image.fromarray(out)


# ---------- движки ----------
def run_tesseract(images, outdir, model):
    """Настоящий WASM-сборка tesseract-wasm под node: время близко к браузеру."""
    work = outdir / "_raw"
    work.mkdir(parents=True, exist_ok=True)
    jobs = []
    for name, img in images.items():
        rgba = img.convert("RGBA")
        p = work / f"{name}.rgba"
        p.write_bytes(rgba.tobytes())
        jobs.append(dict(name=name, path=str(p), w=rgba.width, h=rgba.height))
    (work / "jobs.json").write_text(json.dumps(jobs))
    r = subprocess.run(
        ["node", str(ROOT / "tess_wasm.mjs"), str(work / "jobs.json"), str(model), str(outdir)],
        capture_output=True, text=True,
    )
    if r.returncode:
        sys.exit("tesseract-wasm: " + r.stderr[-800:])
    for j in jobs:
        Path(j["path"]).unlink()
        bp = outdir / f"{j['name']}.boxes.json"
        (outdir / f"{j['name']}.txt").write_text(boxes_to_lines(json.loads(bp.read_text())))
        bp.unlink()
    return json.loads((outdir / "_times.json").read_text())


def boxes_to_lines(items):
    """items: (x0, y0, y1, text). Склейка блоков одной строки по перекрытию по Y."""
    items = sorted(items, key=lambda t: (t[1] + t[2]) / 2)
    rows = []
    for it in items:
        cy, h = (it[1] + it[2]) / 2, it[2] - it[1]
        for row in rows:
            if abs(row["cy"] - cy) < 0.55 * h:
                row["items"].append(it)
                break
        else:
            rows.append(dict(cy=cy, items=[it]))
    return "\n".join(" ".join(t[3] for t in sorted(r["items"])) for r in rows)


def run_ppocr(images, outdir):
    from rapidocr import LangRec, ModelType, OCRVersion, RapidOCR

    eng = RapidOCR(params={
        "Global.log_level": "error",
        "Det.ocr_version": OCRVersion.PPOCRV5, "Det.model_type": ModelType.MOBILE,
        "Rec.ocr_version": OCRVersion.PPOCRV5, "Rec.model_type": ModelType.MOBILE,
        "Rec.lang_type": LangRec.LATIN,
    })
    times = {}
    for name, img in images.items():
        a = np.asarray(img.convert("RGB"))[:, :, ::-1].copy()
        t = time.perf_counter()
        res = eng(a)
        times[name] = time.perf_counter() - t
        items = []
        if res.boxes is not None:
            for b, txt in zip(res.boxes, res.txts):
                items.append((float(b[:, 0].min()), float(b[:, 1].min()), float(b[:, 1].max()), txt))
        (outdir / f"{name}.txt").write_text(boxes_to_lines(items))
    return times


def run_ocrs(images, outdir):
    times = {}
    tmp = outdir / "_tmp.png"
    for name, img in images.items():
        img.save(tmp)
        t = time.perf_counter()
        js = outdir / "_tmp.json"
        subprocess.run([str(CACHE / "ocrs/bin/ocrs"), "--json", "-o", str(js), str(tmp)],
                       capture_output=True, text=True, check=True)
        times[name] = time.perf_counter() - t
        items = []
        for para in json.loads(js.read_text())["paragraphs"]:
            for line in para["lines"]:
                for w in line["words"]:
                    v = w["vertices"]
                    ys = [p[1] for p in v]
                    items.append((min(p[0] for p in v), min(ys), max(ys), w["text"]))
        (outdir / f"{name}.txt").write_text(boxes_to_lines(items))
        js.unlink()
    tmp.unlink(missing_ok=True)
    return times


# ---------- прототип парсера ----------
MONEY = r"(-?\d{1,6}[,.]\s?\d{2})"
# Реальные форматы: "1 op * 12,00 = 12,00 B", "1pcs * 5,00 5,00 A", "3 * 8,00 24,00 A"
ITEM = re.compile(
    r"(?P<name>.*?)\s*(?<![\d,.])(?P<qty>\d+(?:[,.]\d{1,3})?)\s*(?:szt\.?|op\.?|pcs|kg)?\s*"
    r"[xX×*]\s*(?P<price>\d+[,.]\d{2})\s*=?\s*"
    r"(?P<value>\d+[,.]\s?\d{2})\s*(?P<ptu>[A-Ga-g8])?\W*$"
)


def grosze(s):
    s = re.sub(r"\s", "", s).replace(".", ",")
    neg = s.startswith("-")
    a, b = s.lstrip("-").split(",")
    v = int(a) * 100 + int(b)
    return -v if neg else v


def fold(s):
    s = unicodedata.normalize("NFKD", s.replace("ł", "l").replace("Ł", "L"))
    return "".join(c for c in s if not unicodedata.combining(c)).lower()


def sim(a, b):
    return difflib.SequenceMatcher(None, fold(a), fold(b)).ratio()


def parse(text):
    # OCR часто вставляет пробел внутрь суммы: "3, 97" -> "3,97"
    text = re.sub(r"(\d[,.])\s(\d\d)(?!\d)", r"\1\2", text)
    lines = [l.strip() for l in text.splitlines() if l.strip()]
    out = dict(total=None, date=None, nip=None, items=[], head=lines[:10])
    for l in lines:
        if out["total"] is None and re.search(r"S[UV]M[AĄ4]\s*:?\s*P[LI1]N", l, re.I):
            m = re.findall(MONEY, l)
            if m:
                out["total"] = grosze(m[-1])
        if out["date"] is None:
            m = re.search(r"(20\d\d)[-./ ](\d\d)[-./ ](\d\d)", l) or None
            if m:
                out["date"] = "-".join(m.groups())
            else:
                m = re.search(r"(\d\d)[-./](\d\d)[-./](20\d\d)", l)
                if m:
                    out["date"] = f"{m[3]}-{m[2]}-{m[1]}"
        if out["nip"] is None:
            m = re.search(r"NIP[:\s]*([\d\- ]{10,13})", l, re.I)
            if m:
                out["nip"] = re.sub(r"\D", "", m[1])
    prev = ""
    for l in lines:
        m = ITEM.match(l)
        if m:
            name = m["name"].strip()
            # Пустое имя или код товара: настоящее название строкой выше
            if not name or re.fullmatch(r"[\d\s]+", name):
                name = prev
            out["items"].append(dict(
                name=name, qty=m["qty"].replace(".", ","),
                price=grosze(m["price"]), value=grosze(m["value"]),
            ))
        prev = l
    return out


def score(text, t):
    p = parse(text)
    r = dict(
        total=p["total"] == t["total"],
        date=p["date"] == t["date"],
        merchant=max((sim(l, t["merchant"]) for l in p["head"]), default=0) >= 0.85,
        nip=p["nip"] == t.get("nip"),
    )
    pool = list(p["items"])
    ok = 0
    for ti in t["items"]:
        best = max(pool, key=lambda x: sim(x["name"], ti["name"]), default=None)
        if best and sim(best["name"], ti["name"]) >= 0.8 and best["qty"] == ti["qty"] \
                and best["price"] == ti["price"] and best["value"] == ti["value"]:
            ok += 1
            pool.remove(best)
    r["items_ok"], r["items_n"] = ok, len(t["items"])
    r["items_extra"] = len(pool)
    # Польские буквы: выравниваем эталонные строки по лучшей строке OCR
    seen = hit = 0
    lines = [l for l in text.splitlines() if l.strip()]
    for s in [t["merchant"]] + [i["name"] for i in t["items"]]:
        if not any(c in PL for c in s) or not lines:
            seen += sum(c in PL for c in s)
            continue
        b = max(lines, key=lambda l: sim(l, s) if len(l) < 3 * len(s) else sim(l[:len(s) + 8], s))
        sm = difflib.SequenceMatcher(None, s, b, autojunk=False)
        for tag, i1, i2, _, _ in sm.get_opcodes():
            for c in s[i1:i2]:
                if c in PL:
                    seen += 1
                    hit += tag == "equal"
    r["pl_seen"], r["pl_hit"] = seen, hit
    return r


def aggregate(rows):
    n = len(rows)
    if not n:
        return {}
    pct = lambda k: round(100 * sum(r[k] for r in rows) / n, 1)
    ni = sum(r["items_n"] for r in rows)
    ps = sum(r["pl_seen"] for r in rows)
    return dict(
        n=n, total=pct("total"), merchant=pct("merchant"), date=pct("date"), nip=pct("nip"),
        items=round(100 * sum(r["items_ok"] for r in rows) / ni, 1) if ni else None,
        polish=round(100 * sum(r["pl_hit"] for r in rows) / ps, 1) if ps else None,
    )


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("images", type=Path)
    ap.add_argument("--truth", type=Path)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--engines", default="tess_fast,tess_best,ppocr,ocrs")
    a = ap.parse_args()
    files, seen_hash = [], set()
    for p in sorted(p for p in a.images.iterdir() if p.suffix.lower() in IMG_EXT):
        h = hashlib.sha256(p.read_bytes()).digest()
        if h in seen_hash:
            print(f"пропущен дубль: {p.name}")
            continue
        seen_hash.add(h)
        files.append(p)
    if not files:
        sys.exit(f"нет изображений в {a.images}")
    report = dict(n_images=len(files), results={})
    for variant, binar in (("raw", False), ("prep", True)):
        t0 = time.perf_counter()
        images = {p.stem: preprocess(p, binar) for p in files}
        prep_s = (time.perf_counter() - t0) / len(files)
        for eng in a.engines.split(","):
            key = f"{eng}/{variant}"
            d = a.out / eng / variant
            d.mkdir(parents=True, exist_ok=True)
            if eng == "tess_fast":
                times = run_tesseract(images, d, CACHE / "models/pol_fast.traineddata")
            elif eng == "tess_best":
                times = run_tesseract(images, d, CACHE / "models/pol_best.traineddata")
            elif eng == "ppocr":
                times = run_ppocr(images, d)
            elif eng == "ocrs":
                times = run_ocrs(images, d)
            else:
                sys.exit(f"неизвестный движок {eng}")
            rows, per = [], {}
            for name in images:
                tp = a.truth / f"{name}.json" if a.truth else None
                if tp and tp.exists():
                    r = score((d / f"{name}.txt").read_text(), json.loads(tp.read_text()))
                    r["level"] = name.rsplit("_L", 1)[-1] if "_L" in name else "real"
                    rows.append(r)
                    per[name] = r
            res = dict(sec_median=round(statistics.median(times.values()), 2),
                       prep_sec=round(prep_s, 3), all=aggregate(rows))
            for lvl in sorted({r["level"] for r in rows}):
                res[f"L{lvl}"] = aggregate([r for r in rows if r["level"] == lvl])
            report["results"][key] = res
            (d / "per_receipt.json").write_text(json.dumps(per, indent=1))
            print(key, json.dumps(res["all"], ensure_ascii=False), f"{res['sec_median']}s", flush=True)
    (a.out / "report.json").write_text(json.dumps(report, indent=1, ensure_ascii=False))


if __name__ == "__main__":
    main()
