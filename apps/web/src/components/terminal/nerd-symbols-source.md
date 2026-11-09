# Terminal prompt icons

`nerd-symbols.woff2` is a private-use-only subset of Nerd Fonts **v3.5.1**
[JetBrainsMonoNerdFontMono-Regular.ttf](https://github.com/ryanoasis/nerd-fonts/blob/v3.5.1/patched-fonts/JetBrainsMono/Ligatures/JetBrainsMonoNerdFontMono-Regular.ttf).
The source SHA-256 is `f2a5ea6cfab397445ffab00c0370927b66d61e560a05db5db271b42006381c1a`.

The Mono variant gives icons a single terminal cell. Only private-use characters
are retained, so the terminal's existing Latin, CJK, and standard-symbol fonts
continue to draw ordinary text. The resulting WOFF2 contains 10,610 mapped
characters and is 922,960 bytes. It is loaded with the terminal, with a
content-hashed asset URL for caching; Chat and the surrounding UI do not use it.
The `/next` terminal bundle embeds the same font and is also loaded on demand.

To reproduce with fontTools 4.39.2 and Brotli 1.2.1:

```python
from fontTools import subset
from fontTools.ttLib import TTFont

font = TTFont("JetBrainsMonoNerdFontMono-Regular.ttf")
private_use = {
    cp for cp in font.getBestCmap()
    if 0xE000 <= cp <= 0xF8FF
    or 0xF0000 <= cp <= 0xFFFFD
    or 0x100000 <= cp <= 0x10FFFD
}
options = subset.Options()
options.name_IDs = ["*"]
options.name_legacy = True
options.name_languages = ["*"]
subsetter = subset.Subsetter(options=options)
subsetter.populate(unicodes=private_use)
subsetter.subset(font)
font.flavor = "woff2"
font.save("nerd-symbols.woff2")
```

Upstream notices are preserved in `nerd-symbols-upstream.md`,
`nerd-symbols-OFL.txt`, and `nerd-symbols-LICENSE.txt`. This modification only
subsets and compresses the font; it does not change icon artwork or metrics.
