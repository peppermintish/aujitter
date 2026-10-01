"""Render the repository's pulse mark to installer sizes. Requires Pillow."""
from pathlib import Path
from PIL import Image, ImageDraw

out = Path(__file__).resolve().parents[1] / "packaging/assets"
out.mkdir(parents=True, exist_ok=True)
image = Image.new("RGBA", (1024, 1024), (0, 0, 0, 0))
draw = ImageDraw.Draw(image)
draw.rounded_rectangle((0, 0, 1023, 1023), radius=224, fill="#0f172a")
draw.line([(144, 536), (336, 536), (424, 328), (544, 736), (648, 448), (720, 536), (880, 536)], fill="#22d3ee", width=48, joint="curve")
for filename, size in [("icon1024.png", 1024), ("Square150x150Logo.png", 150), ("Square44x44Logo.png", 44), ("StoreLogo.png", 50)]:
    image.resize((size, size), Image.Resampling.LANCZOS).save(out / filename)
image.save(out / "aujitter.ico", sizes=[(16,16), (32,32), (48,48), (256,256)])
