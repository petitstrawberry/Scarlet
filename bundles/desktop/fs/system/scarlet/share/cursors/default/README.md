# Scarlet Default Cursor Theme

`theme.toml` is the runtime manifest. It declares the theme's source-image
density and maps each SWS cursor state to a PNG and source-pixel hotspot. The
bundled images are 42 x 56 pixel 2x assets, producing a 21 x 28 logical cursor
at the default output scale.

The theme currently provides `arrow`, `pointer`, `text`, `crosshair`, `move`,
four resize directions, `wait`, and `not_allowed`. A theme may reuse a complete
image definition, including its hotspot, without duplicating files:

```toml
[crosshair]
alias = "arrow"
```

Aliases may refer forward or chain through other aliases. SWS rejects missing
targets and cycles. A normal filesystem symlink can also be used as an image.

`arrow.svg` keeps the source polygon coordinates from SVG Repo's CC0 `Mouse
Arrow` (ID 168135); only its presentation and canvas are adapted. Its left edge
is vertical from the tip to the lower corner. The remaining theme geometry was
created for Scarlet and uses the same neutral dark fill, white outline, and
subtle shadow.

- Arrow source page: https://www.svgrepo.com/svg/168135/mouse-arrow
- Original SVG: https://www.svgrepo.com/show/168135/mouse-arrow.svg
- Original SVG SHA-256:
  `b2bb6118c6ce5bc47c96d9bfad92a500b9e2949fdddb7e2594a12e67edf20faa`
- Arrow source license: Creative Commons CC0 1.0 / public domain
