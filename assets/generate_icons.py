"""Rebuild PNG/ICO exports from the editable logo SVG. Requires cairosvg and Pillow."""
from pathlib import Path
from io import BytesIO
import xml.etree.ElementTree as ET

import cairosvg
from PIL import Image

ROOT = Path(__file__).resolve().parent
ferris = ET.parse(ROOT / "ferris.svg").getroot()
crab = "".join(ET.tostring(child, encoding="unicode") for child in ferris)
svg = '''<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256" viewBox="0 0 256 256">
  <title>Rusty Launcher — Ferris on a Minecraft grass block</title>
  <defs>
    <clipPath id="left"><path d="M32 145 128 193 128 249 32 201Z"/></clipPath>
    <clipPath id="right"><path d="M128 193 224 145 224 201 128 249Z"/></clipPath>
    <clipPath id="top"><path d="M128 97 224 145 128 193 32 145Z"/></clipPath>
  </defs>
  <path d="M128 97 224 145 224 201 128 249 32 201 32 145Z" fill="#32251f" stroke="#111a15" stroke-width="9" stroke-linejoin="round"/>
  <path d="M32 145 128 193 128 249 32 201Z" fill="#926443"/>
  <path d="M128 193 224 145 224 201 128 249Z" fill="#67482f"/>
  <g clip-path="url(#left)" fill="#b38256">
    <path d="M34 179h15v14H34zM64 190h14v14H64zM95 219h16v14H95zM47 208h14v13H47zM109 197h16v12h-16z"/>
  </g>
  <g clip-path="url(#left)" fill="#6e4c35">
    <path d="M49 179h15v14H49zM79 205h16v14H79zM111 235h17v14h-17zM32 193h16v13H32z"/>
  </g>
  <g clip-path="url(#right)" fill="#856040">
    <path d="M143 206h16v14h-16zM174 190h16v14h-16zM205 174h20v14h-20zM174 219h16v14h-16z"/>
  </g>
  <path d="M32 145 128 193 128 218 112 210 112 202 96 194 96 201 80 193 80 180 64 172 64 184 48 176 48 165 32 157Z" fill="#5e963d"/>
  <path d="M128 193 224 145 224 161 208 169 208 181 192 189 192 181 176 189 176 202 160 210 160 202 144 210 144 218 128 226Z" fill="#3e732f"/>
  <path d="M128 97 224 145 128 193 32 145Z" fill="#83b84a"/>
  <g clip-path="url(#top)" fill="#a3ce60">
    <path d="m48 137 16-8 16 8-16 8Zm32 16 16-8 16 8-16 8Zm32-32 16-8 16 8-16 8Zm32 32 16-8 16 8-16 8Zm32-16 16-8 16 8-16 8Zm-64 48 16-8 16 8-16 8Z"/>
  </g>
  <path d="m32 145 96 48 96-48M128 193v56" fill="none" stroke="#1c2b1c" stroke-opacity=".35" stroke-width="3"/>
  <g transform="translate(9 -7) scale(.1983)">''' + crab + '''</g>
</svg>
'''
(ROOT / "logo.svg").write_text(svg, encoding="utf-8")
png = cairosvg.svg2png(bytestring=svg.encode(), output_width=512, output_height=512)
(ROOT / "logo.png").write_bytes(png)
Image.open(BytesIO(png)).save(ROOT / "app.ico", sizes=[(16,16),(24,24),(32,32),(48,48),(64,64),(128,128),(256,256)])
print("Generated logo.svg, logo.png, app.ico")
