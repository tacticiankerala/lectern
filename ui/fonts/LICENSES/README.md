# Bundled fonts

Every font here is under the SIL Open Font License 1.1, and each family's licence text sits next to
this file. All of them come from the projects' official releases.

## Sources

- **Inter 4.1** (font version 4.001)
  - From https://github.com/rsms/inter/releases/download/v4.1/Inter-4.1.zip: `InterVariable.ttf`
    and `InterVariable-Italic.ttf`.
  - Bundled as `Inter-Variable.woff2` and `Inter-Variable-Italic.woff2` (subset).
- **Atkinson Hyperlegible Next 2.001**
  - From https://github.com/googlefonts/atkinson-hyperlegible-next at commit
    `7925f50f649b3813257faf2f4c0b381011f434f1`: `fonts/variable/AtkinsonHyperlegibleNext[wght].ttf`
    and `fonts/variable/AtkinsonHyperlegibleNext-Italic[wght].ttf`, the files Google Fonts ships.
    The project publishes no release archive.
  - Bundled as `AtkinsonHyperlegibleNext-Variable.woff2` and
    `AtkinsonHyperlegibleNext-Variable-Italic.woff2` (subset).
- **Literata 3.103**
  - From https://github.com/googlefonts/literata/releases/download/3.103/3.103.zip:
    `fonts/variable/Literata[opsz,wght].ttf` and `fonts/variable/Literata-Italic[opsz,wght].ttf`.
  - Bundled as `Literata-Variable.woff2` and `Literata-Variable-Italic.woff2` (subset).
- **Source Serif 4 4.005**
  - From https://github.com/adobe-fonts/source-serif/releases/download/4.005R/source-serif-4.005_WOFF2.zip:
    `VAR/SourceSerif4Variable-Roman.ttf.woff2` and `VAR/SourceSerif4Variable-Italic.ttf.woff2`.
  - Bundled as those two files, unmodified.
- **JetBrains Mono 2.304**
  - From https://github.com/JetBrains/JetBrainsMono/releases/download/v2.304/JetBrainsMono-2.304.zip:
    `fonts/variable/JetBrainsMono[wght].ttf` and `fonts/variable/JetBrainsMono-Italic[wght].ttf`.
  - Bundled as `JetBrainsMono-Variable.woff2` and `JetBrainsMono-Variable-Italic.woff2` (subset).

## Subsetting

Inter, Atkinson Hyperlegible Next, Literata and JetBrains Mono were subset with fonttools 4.66.1,
keeping every variable axis:

```sh
pyftsubset <font>.ttf --flavor=woff2 \
  --unicodes="U+0000-00FF,U+0100-017F,U+2000-206F,U+2190-21FF,U+2200-22FF,U+2500-257F,U+25A0-25FF,U+02C6,U+02DA,U+02DC,U+20AC,U+2122,U+2303,U+2318,U+2325,U+FFFD" \
  --layout-features="kern,liga,calt,ccmp,locl,mark,mkmk,rvrn,tnum,lnum,pnum,onum,case"
```

JetBrains Mono takes `--layout-features="ccmp,locl,mark,mkmk,rvrn"` instead. Lectern turns code
ligatures off, so their glyphs would never show, and leaving them out makes the default code font,
which loads at every start, about a third smaller and quicker to decode.

The ranges are Basic Latin, Latin-1, Latin Extended-A, General Punctuation, Arrows, Mathematical
Operators, Box Drawing and Geometric Shapes. The extra code points are the spacing accents, euro,
trade mark and replacement character of the usual web "latin" set, plus the ⌘ ⌥ ⌃ key symbols.

Source Serif 4 is not subset. Its licence reserves the font name "Source", and the OFL forbids a
modified version (which a subset is) from using a reserved name without Adobe's permission. So
Lectern ships Adobe's own WOFF2 files exactly as released.
