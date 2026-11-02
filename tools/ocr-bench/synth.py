"""Generator of SYNTHETIC Polish receipts for checking the bench itself.

This does not replace real photos: print, paper and camera are all artificial.
Synthetic results never count toward the project's accuracy targets.
All shops, NIPs and products are made up.

    python synth.py OUT_DIR [--n 24] [--seed 7]
"""
import argparse
import json
import random
from pathlib import Path

from PIL import Image, ImageDraw, ImageEnhance, ImageFilter, ImageFont
import numpy as np

FONTS = [
    "/System/Library/Fonts/Menlo.ttc",
    "/System/Library/Fonts/Supplemental/Courier New.ttf",
    "/System/Library/Fonts/Supplemental/Arial.ttf",
]
SHOPS = [
    ("Sklep Żabka Testowa", "ul. Świętokrzyska 12, 00-116 Łódź"),
    ("Delikatesy Pod Różą", "ul. Źródlana 5, 30-001 Kraków"),
    ("Market Gęś Sp. z o.o.", "al. Piłsudskiego 44, 80-210 Gdańsk"),
    ("Drogeria Jaśmin", "ul. Ogrodowa 3/7, 61-821 Poznań"),
]
GOODS = [
    ("Chleb żytni krojony 500g", "C"), ("Mleko 3,2% UHT 1l", "C"),
    ("Jabłka luz", "C"), ("Masło ekstra 200g", "C"), ("Śmietana 18% 330g", "C"),
    ("Ziemniaki młode", "C"), ("Szampon łagodzący 400ml", "A"),
    ("Proszek do prania 1,5kg", "A"), ("Płyn do naczyń cytrynowy", "A"),
    ("Pierogi z kapustą 400g", "C"), ("Woda niegaz. 1,5l", "A"),
    ("Ręczniki papierowe 2szt", "A"), ("Łosoś wędzony 100g", "C"),
    ("Żurek w butelce 500ml", "C"), ("Herbata miętowa 20szt", "A"),
]
RATES = {"A": 2300, "B": 800, "C": 500}


def fmt(g):
    return f"{g // 100},{g % 100:02d}"


def make_receipt(rng):
    shop, addr = rng.choice(SHOPS)
    nip = "".join(rng.choice("0123456789") for _ in range(10))
    date = f"2026-{rng.randint(1, 10):02d}-{rng.randint(1, 28):02d}"
    tm = f"{rng.randint(7, 21):02d}:{rng.randint(0, 59):02d}"
    items, lines = [], []
    for name, ptu in rng.sample(GOODS, rng.randint(4, 11)):
        weighed = name.endswith("luz") or name.startswith("Ziemniaki")
        qty_milli = rng.randint(180, 2400) if weighed else rng.randint(1, 3) * 1000
        price = rng.randint(149, 2499)
        value = (qty_milli * price + 500) // 1000
        qty_s = f"{qty_milli / 1000:.3f}".replace(".", ",") if weighed else str(qty_milli // 1000)
        lines.append((f"{name}", f"{qty_s} x{fmt(price)}", f"{fmt(value)} {ptu}"))
        item = dict(name=name, qty=qty_s, price=price, value=value, ptu=ptu, discount=0)
        if rng.random() < 0.15:
            d = rng.randint(50, min(300, value - 1))
            lines.append(("RABAT", "", f"-{fmt(d)} {ptu}"))
            item["discount"] = d
        items.append(item)
    total = sum(i["value"] - i["discount"] for i in items)
    by = {}
    for i in items:
        by[i["ptu"]] = by.get(i["ptu"], 0) + i["value"] - i["discount"]
    ptu_lines = []
    tax_total = 0
    for k, gross in sorted(by.items()):
        r = RATES[k]
        tax = (gross * r + (10000 + r) // 2) // (10000 + r)
        tax_total += tax
        ptu_lines.append((f"Sprzedaż opodatk. {k}", "", fmt(gross)))
        ptu_lines.append((f"Kwota PTU {k} {r / 100:.2f}%".replace(".", ","), "", fmt(tax)))
    pay = rng.choice(["Karta płatnicza", "Gotówka"])
    truth = dict(merchant=shop, nip=nip, date=date, time=tm, total=total, payment=pay, items=items)
    return truth, addr, lines, ptu_lines


def render(truth, addr, lines, ptu_lines, rng):
    font_path = rng.choice(FONTS)
    size = 26
    font = ImageFont.truetype(font_path, size)
    W = 640
    rows = []  # (left, right, centered)
    c = lambda s: rows.append((s, "", True))
    r = lambda a, b="": rows.append((a, b, False))
    c(truth["merchant"]); c(addr); c(f"NIP {truth['nip']}")
    r(truth["date"], f"nr wydr. {rng.randint(10000, 99999)}")
    c("PARAGON FISKALNY")
    for name, mid, val in lines:
        if mid:
            r(name)
            r("   " + mid, val)
        else:
            r(name, val)
    r("-" * 40)
    for a, _, b in ptu_lines:
        r(a, b)
    r("SUMA PTU", fmt(sum(int(b.replace(",", "")) for a, _, b in ptu_lines if a.startswith("Kwota"))))
    r("SUMA PLN", fmt(truth["total"]))
    r(truth["payment"], fmt(truth["total"]))
    r(f"{truth['date']} {truth['time']}", f"#{rng.randint(1000, 9999)}")
    H = 40 + (size + 10) * len(rows) + 40
    img = Image.new("L", (W, H), 245)
    d = ImageDraw.Draw(img)
    y = 40
    for a, b, center in rows:
        if center:
            d.text(((W - d.textlength(a, font=font)) / 2, y), a, font=font, fill=25)
        else:
            d.text((28, y), a, font=font, fill=25)
            if b:
                d.text((W - 28 - d.textlength(b, font=font), y), b, font=font, fill=25)
        y += size + 10
    return img


def degrade(img, level, rng):
    """level: 0 clean, 1 photo, 2 faded or crumpled."""
    np_rng = np.random.default_rng(rng.randint(0, 2**31))
    bg = 200 if level else 235
    canvas = Image.new("L", (img.width + 200, img.height + 200), bg - 90 if level else bg)
    canvas.paste(img, (100, 100))
    if level:
        canvas = canvas.rotate(rng.uniform(-3.5, 3.5), resample=Image.BICUBIC, fillcolor=110)
        canvas = canvas.filter(ImageFilter.GaussianBlur(0.6 * level))
    a = np.asarray(canvas, dtype=np.float32)
    if level >= 2:
        a = 255 - (255 - a) * rng.uniform(0.35, 0.55)  # faded thermal paper
        xx = np.linspace(0, 1, a.shape[1])[None, :]
        yy = np.linspace(0, 1, a.shape[0])[:, None]
        a = a * (0.72 + 0.28 * (xx * 0.6 + yy * 0.4))  # uneven light
    if level:
        a = a + np_rng.normal(0, 4 * level, a.shape)
    out = Image.fromarray(np.clip(a, 0, 255).astype(np.uint8))
    return ImageEnhance.Contrast(out).enhance(1.0)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out", type=Path)
    ap.add_argument("--n", type=int, default=24)
    ap.add_argument("--seed", type=int, default=7)
    a = ap.parse_args()
    rng = random.Random(a.seed)
    a.out.mkdir(parents=True, exist_ok=True)
    (a.out / "truth").mkdir(exist_ok=True)
    for i in range(a.n):
        level = i % 3
        t, addr, lines, ptu = make_receipt(rng)
        img = degrade(render(t, addr, lines, ptu, rng), level, rng)
        name = f"synth_{i:03d}_L{level}"
        img.save(a.out / f"{name}.jpg", quality=88)
        (a.out / "truth" / f"{name}.json").write_text(json.dumps(t, ensure_ascii=False, indent=1))


if __name__ == "__main__":
    main()
