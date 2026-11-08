#!/usr/bin/env python3
"""Genera assets/buscatxt.ico y assets/buscatxt.png (documento con lupa).

Solo hace falta para cambiar el diseno del icono; la compilacion usa los
archivos ya generados. Necesita Pillow: python3 -m pip install pillow
"""
from pathlib import Path

from PIL import Image, ImageDraw

ASSETS = Path(__file__).resolve().parent.parent / "assets"
S = 1024
AZUL = (30, 94, 196, 255)
NARANJA = (255, 176, 32, 255)

img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
d = ImageDraw.Draw(img)

# Fondo: cuadrado redondeado azul
d.rounded_rectangle((40, 40, S - 40, S - 40), radius=200, fill=AZUL)

# Documento blanco con la esquina doblada
x0, y0, x1, y1, fold = 200, 150, 680, 830, 140
d.polygon([(x0, y0), (x1 - fold, y0), (x1, y0 + fold), (x1, y1), (x0, y1)], fill=(255, 255, 255, 255))
d.polygon([(x1 - fold, y0), (x1 - fold, y0 + fold), (x1, y0 + fold)], fill=(190, 205, 230, 255))

# Lineas de texto
for i, w in enumerate([340, 300, 340, 220, 300]):
    y = 340 + i * 90
    d.rounded_rectangle((270, y, 270 + w, y + 36), radius=18, fill=(150, 170, 205, 255))

# Lupa
cx, cy, r = 610, 560, 190
d.line((cx + 120, cy + 120, 880, 880), fill=NARANJA, width=110)
d.ellipse((880 - 55, 880 - 55, 880 + 55, 880 + 55), fill=NARANJA)
d.ellipse((cx - r, cy - r, cx + r, cy + r), fill=NARANJA)
r2 = r - 55
d.ellipse((cx - r2, cy - r2, cx + r2, cy + r2), fill=(225, 238, 255, 255))

ASSETS.mkdir(exist_ok=True)
img.save(ASSETS / "buscatxt.ico", sizes=[(s, s) for s in (16, 24, 32, 48, 64, 128, 256)])
img.resize((256, 256), Image.LANCZOS).save(ASSETS / "buscatxt.png")
