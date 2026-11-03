"""Draws the synthetic receipt used by the end-to-end tests. Shop, NIP and products are made up.

    tools/ocr-bench/.cache/venv/bin/python e2e/fixtures/make.py
"""
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

OUT = Path(__file__).with_name("receipt.jpg")
FONT = ImageFont.truetype("/System/Library/Fonts/Menlo.ttc", 30)
W = 760
ROWS = [
    ("c", "Sklep Testowy Sp. z o.o."),
    ("c", "ul. Przykładowa 1, 00-001 Warszawa"),
    ("c", "NIP 123-456-32-18"),
    ("lr", "2026-10-31", "nr wydr. 000123"),
    ("c", "PARAGON FISKALNY"),
    ("l", "Chleb żytni 500g"),
    ("lr", "   1 x4,99", "4,99 C"),
    ("l", "Szampon łagodzący 400ml"),
    ("lr", "   2 x12,50", "25,00 A"),
    ("l", "Mleko 3,2% 1l"),
    ("lr", "   3 x3,29", "9,87 C"),
    ("lr", "RABAT", "-1,00 C"),
    ("l", "-" * 34),
    ("lr", "Sprzedaż opodatk. A", "25,00"),
    ("lr", "Kwota PTU A 23,00%", "4,67"),
    ("lr", "Sprzedaż opodatk. C", "13,86"),
    ("lr", "Kwota PTU C 5,00%", "0,66"),
    ("lr", "SUMA PTU", "5,33"),
    ("lr", "SUMA PLN", "38,86"),
    ("lr", "Karta płatnicza", "38,86"),
    ("lr", "2026-10-31 12:15", "#0042"),
]


def main():
    step = 46
    paper = Image.new("L", (W, 80 + step * len(ROWS)), 244)
    d = ImageDraw.Draw(paper)
    for i, row in enumerate(ROWS):
        y = 40 + i * step
        if row[0] == "c":
            d.text(((W - d.textlength(row[1], font=FONT)) / 2, y), row[1], font=FONT, fill=30)
        else:
            d.text((36, y), row[1], font=FONT, fill=30)
            if row[0] == "lr":
                d.text((W - 36 - d.textlength(row[2], font=FONT), y), row[2], font=FONT, fill=30)
    scene = Image.new("L", (W + 240, paper.height + 240), 70)
    scene.paste(paper, (120, 120))
    scene = scene.rotate(2.5, resample=Image.BICUBIC, fillcolor=70).filter(ImageFilter.GaussianBlur(0.7))
    scene.convert("RGB").save(OUT, quality=88)


if __name__ == "__main__":
    main()
