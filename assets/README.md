# assets

`Inter-Medium.ttf` is the font the island's toast is drawn with. It is Inter
4.1 Medium subset to Latin-1 plus the punctuation the messages use, which is
what takes it from 417 KB to 16 KB - the daemon renders a handful of fixed
English strings and has no use for the rest.

Vendored rather than loaded from the system so the toast looks the same on
every machine and needs no fontconfig in a daemon that otherwise has no UI
stack at all. Licensed under the SIL Open Font License 1.1, see
`Inter-LICENSE.txt`.

Regenerate with:

    pyftsubset Inter-Medium.ttf \
      --unicodes="U+0020-007E,U+00A0-00FF,U+2018-201D,U+2026,U+2013,U+2014" \
      --layout-features='' --no-hinting --desubroutinize \
      --output-file=assets/Inter-Medium.ttf
