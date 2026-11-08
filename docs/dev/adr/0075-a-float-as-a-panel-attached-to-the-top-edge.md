# 0075. A float as a panel attached to the top edge

- Status: Accepted, amended by 0076
- Date: 2026-11-08

## Context

The author tried the float of [ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md) at the
keyboard and found it dated: a 2 px light border, square corners, no shadow, a fill much lighter than the
text, and a place one row below the top. From four directions drawn as a mock-up, the author chose
**A: a panel attached to the top edge**, close to VS Code's Quick Input:

- it hangs from the top edge of the text area, centered, with only its bottom corners rounded;
- a soft shadow and a hairline of low contrast lift it off the text, with no border;
- its fill is only a little lighter than the text area;
- the label has a row of its own, and an answer is typed in a field with a thin accent outline on the
  text area's background;
- a question shows its keys as small chips: `y exit`, `n stay`;
- padding is in pixels, not whole cells, where the renderer allows it.

What the renderer allows today:

- Every pane is a rectangle of whole cells (ADR 0061). Rectangles are one pipeline of flat quads with
  one color each, and glyphs another, both in one render pass, one draw each per layer.
- The grid starts after a margin of 8 physical pixels on the top and the left (`frame::PixelGrid`),
  at every scale. A cell of 13 pt SF Mono is about 8 x 16 px at scale 1 and twice that at scale 2.
- Glyphs sit on whole pixels at their cell, and the IME's cursor area, the text-grid snapshots and
  later the mouse all map cells to pixels through the same grid.

ADR 0072's rules stay: panes in a float draw only inside it, a passphrase is masked, `(y or n)` (now the
keys) always shows whole, nested prompts show `[2]`, nothing of a secret buffer goes over the socket,
the text beneath keeps drawing, and a frame with no float costs nothing more.

## Decision

The author accepted this ADR on 2026-11-08, through the coordinator, with the public API it changes
in `oso-render` and `oso-core`.

The author chose, through the coordinator, on 2026-11-08, the option described for each point marked
**(chosen)**, from choices put to them with the others, which are under
[Alternatives considered](#alternatives-considered): the cell grid with chrome in pixels, the drawing in
a second pipeline, the public API below, the hairline in points, the chips with verbs, the order for
small frames, and the color tokens. They also accepted that row 0 of the text is hidden while a float is
open, and, after the reviews, that a label too tall for a short float keeps its first rows and that
the save-over question opens with what it asks and the file's name. From screenshots of three variants taken in the app, they then picked the values marked
**(picked)**: the middle one, the mock-up's, with a lighter key chip.

### Where it sits

- **Attached to the top edge.** The float's cells start on the text area's first row (no row above it
  any more), and its panel covers the top margin above them up to the surface's edge. Row 0 of the
  window beneath is hidden under it, accents in the margin included.
- **Width as before:** 60% of the columns, at least 40, never closer than 2 columns to an edge.

### Text stays on the cell grid, the chrome is in pixels (chosen)

- **What is inside a float is drawn in whole cells, as any pane is:** the label, the answer, the
  message and the keys. The cursor box, the IME's cursor area, the bands of ADR 0052, the snapshots and
  M2-17's candidate rows need nothing new.
- **Each row of padding goes to the panes of one row at most:** the glyphs of the answer's row, or
  the keys', keep their bands into the gap above and the padding below, and the label keeps to its
  own rows, so that no glyph of one pane of the float draws over another's (ADR 0072).
- **The panel, the field and the chips are drawn in pixels around those cells,** in padding that is
  whole cells, so the panel's edges fall on cell edges (no strip of a half-covered row shows below it)
  and only the field and the chips sit off the grid, inside the padding.
- **The cells of a float that reads an answer**, from the top:

  | Rows | What | Pixels at scale 1 (13 pt) |
  |---|---|---|
  | margin | the top margin, inside the panel | 8 |
  | 1 to 3 | the label, cut with `…` as before | 16 each |
  | 1 | a gap, which the field reaches 4 px into | 16 |
  | 1 | the answer, in its field, with a message at its end | 16 |
  | 1 | the bottom padding, which the field reaches 4 px into | 16 |

  and 2 columns of padding on each side (16 px), which the field reaches 6 px into. A float with a
  one-row label is 4 rows and the margin: 72 px at scale 1, against the mock-up's 78. The field is
  24 px tall, with 12 px between it and the panel's edge below, and the label's last row ends 12 px
  above it. ADR 0072's float was 3 rows below a row of text: this one covers one row more, row 0.
- **A question** is the same with the keys' row in the answer's place: the label, the gap, the keys, the
  bottom padding. A question opened from a prompt then shows at the same height.
- **Where the label goes:** the whole width inside the padding, not the width less 20 columns, as the
  answer no longer shares its row.
- **The top margin is 8 physical pixels at every scale,** so the panel's padding above the label is
  8 px at scale 1 and 4 pt at scale 2, where the empty top of the cell adds about 3 pt more. Screenshots
  at scale 2 decide whether that is enough; if not, the float takes the gap row above the label too.

### The panel

- **One shape per frame,** drawn in pixels in `Layer::FLOAT`: a fill in `Face::Float`, its bottom
  corners rounded, a hairline along its sides and bottom, and a shadow below and to the sides. Its top
  edge is the surface's top edge, square, with no hairline.
- **Sizes are in points,** where a point is the cell's height over 16, as `line_width` already takes
  it, so they follow the font size as well as the scale:

  | | Points | Scale 1 | Scale 2 |
  |---|---|---|---|
  | Corner radius (picked, of 6, 8 and 12) | 8 | 8 px | 16 px |
  | Hairline (chosen) | 1, outside the fill | 1 px | 2 px |
  | Shadow offset down | 10 | 10 px | 20 px |
  | Shadow blur, sigma | 14 (CSS's 28 px) | 14 px | 28 px |
  | Field outline | 1 | 1 px | 2 px |
  | Field radius | 5 | 5 px | 10 px |
  | Chip radius | 4 | 4 px | 8 px |

  Radii and the shadow are in fractional pixels with 1 px of antialiasing. The hairline and outlines
  are whole pixels, at least 1.
- **The shadow and the hairline stay inside the text area of the window beneath:** the panel pane's
  region is that text area, and nothing of it reaches the modeline or the echo row. With less room
  below the panel than the shadow reaches, it shrinks and fades to fit, so it ends before the text
  area does, not in a hard line along it; with none, there is no shadow.
- **No corner is wider than half its box,** so a narrow panel, field or chip stays a box.
- **The text beneath keeps drawing,** under the shadow, which only darkens it.

### The field and the chips

- **The field** is the answer's row, widened 6 points into the side padding and 4 points into the
  gap and the bottom padding, but only into the padding there is: `oso-core` gives the cells it may
  reach into (`PromptLayout::field_room`), and its box, outline included, is cut to them. It is
  filled in `Face::Field`'s background, the text's, with a 1-point outline in its foreground. A
  passphrase's `*` and the cursor box are in it as before.
- **The chips (chosen):** a question shows its keys on their own row, right-aligned:
  `y exit  n stay`. Each key is 3 cells, ` y `, on a chip of those cells inset 2 points top and
  bottom and 1 point at the sides, in `Face::Key`, followed by a space and its verb in the label's dim
  face. Pairs are 2 cells apart. The app draws each key and verb at the cells `minibuffer::layout`
  gives it, as the snapshots do, so a chip and its letter cannot drift apart.
- **The verbs come from the question (chosen):** `Editor::ask_with` takes them with the command
  `y` runs, as `&'static str`, so a command names what each key does: the exit guard `exit` and
  `stay`, saving over a changed file `save over` and `keep`. A question with none shows `yes` and
  `no`.
- **The keys always show whole,** which is ADR 0072's `(y or n)` rule: they have a row the label never
  takes, the verbs go before the keys when the row is narrow, and only a float narrower than the two
  keys (7 cells) cuts them at its edge, as `(y or n)` was cut before.
- **`Answer y or n`** shows at the start of the keys' row, in the message's face, and goes at the next
  key as before.
- `get-state`'s `prompt` keeps the label as asked, `(y or n)` included, so the socket and the flows
  read what they read today.
- **A question opens with what it asks, a name from a file right after (chosen):** saving over a
  changed file asks `Save over notes.md? It changed on disk since it was read.`, not ADR 0072's
  `notes.md changed on disk since it was read; save over it?`, so that the question's opening and
  the file it names always show, as far as the row holds the name. A word too long for any row of
  a label, such as a long name, starts where it is instead of on a row of its own.

### Small frames (chosen)

The text area of the window beneath has `t` rows, and the float with an `L`-row label wants `L + 3`.
It gives up, in this order, and is never taller than the text area:

1. the bottom padding row: the panel ends at the text area's bottom, and the field keeps to the
   answer's row there;
2. the gap row: the field shrinks to the answer's row;
3. the label's last rows, keeping its first, the last of them cut with `…` (chosen), so that the
   question's opening and its depth, `[2]`, always show. ADR 0072 kept the last row for its
   `(y or n)`, which the keys' row now holds;
4. at `t = 1`, ADR 0072's one-row form: the label's first row and then the answer, or the keys' chips
   with no verbs, on one row, on the panel with no field. The label stops before the keys, even for a
   cluster wider than its room.

A frame of no rows shows nothing of the float.

So a one-row label has the full panel from 6 frame rows up (ADR 0072's ring went at 4 rows), and a
frame of 3 rows (one text row) shows the one-row form. Narrow frames keep ADR 0072's widths. Below
12 columns the side padding goes to one column each, and below 5 to none, with the field's outline
inside the answer's cells.

### How oso-render draws it

- **Rounded boxes are a second pipeline in `shape`,** in the same render pass, with no new pass and no
  blur pass: one instanced quad per box, whose fragment shader takes the box's signed distance for the
  corners and the outline, and a closed-form approximation of a Gaussian blur of a rounded box for the
  shadow (Evan Wallace's, as several GPU UIs use). An instance holds the box, four corner radii, the
  fill, the outline's width and color, the shadow's offset, sigma and color, and its quad; 112 bytes.
- **The geometry is the scene's:** `scene::panel` turns a `Panel` into boxes, with colors from the
  faces and sizes from a `PanelStyle` the caller passes, and **cuts each quad to the pane's region on
  the CPU**, as ADR 0061 clips everything else, so a scene test checks that nothing passes it. The GPU
  draws the quad as it is given.
- **Order in a layer:** flat rectangles, then rounded boxes, then glyphs. A float's panel, field and
  chips are in `Layer::FLOAT`, under the content, so the cursor box in `Layer::FLOAT_CONTENT` stays
  above the field.
- **Cost per frame with a float open:** two to four instances (panel, field or two chips), at most 448
  bytes uploaded, and one more draw call in one layer. The panel's quad covers the panel and its shadow,
  about 1900 x 250 physical pixels on a 2880 px wide window at scale 2, under 0.5 Mpx. The shadow is
  about 200 operations a fragment, some 20 of them transcendental, and is skipped under the opaque
  fill, about half the quad; only the bottom margin adds the shadow's offset. That is estimated well
  under 0.1 ms of GPU time on an M1, to be measured with a GPU capture. The flat background and border
  rectangles of ADR 0072 go, so the layer's draw calls do not grow. Building the boxes on the CPU is
  constant, with no allocation.
- **At startup** the second pipeline is compiled with the first, before the first frame: about 1 to 3
  ms, which `just perf`'s `startup_ms` checks. If it shows, the pipeline is created after the first
  frame, not on first use, which would move the stall into the key that opens a prompt.
- **With no float open nothing changes:** the instance list is empty, nothing is uploaded and no draw
  is recorded. The flat rectangles keep their shader and vertices.
- **Colors stay linear and blending premultiplied,** as the glyphs' are.

### The public API (chosen)

- `Pane::border: Option<Face>` goes, for `Pane::panel: Option<Panel<'a>>`:

  ```rust
  /// A float's panel (ADR 0075), drawn in pixels around cells and cut to the pane's region.
  pub struct Panel<'a> {
      /// The cells the panel covers. On the frame's top row it also covers the margin above.
      pub cells: CellRect,
      /// The answer's cells, drawn as an input field.
      pub field: Option<CellRect>,
      /// The cells the field may reach into past its own; its box is cut to them.
      pub field_room: CellRect,
      /// Cells drawn as key chips, such as ` y ` and ` n `.
      pub keys: &'a [CellRect],
  }
  ```

  The pane's region is the text area of the window beneath, which the shadow stays in, and its
  `background` is `None`.
- `Face::Field` (the field's fill in its background, its outline in its foreground) and `Face::Key`
  (a chip and its letter). `Face::FloatBorder`
  becomes the hairline, and `Face::Prompt` the label's dim face.
- In `oso-core`: `Editor::ask_with` takes the two verbs, and `Editor::ask` gives `yes` and `no`
  (`minibuffer::VERBS`); `Kind::Question` gains `verbs`; `PromptLayout` gains `label_cells`, `field`,
  `field_room`, `keys` and `verbs` and loses `suffix`; `FloatRegions` gains `gap` and `padded_below` and loses `framed`; and
  `minibuffer::KEYS` and `KEY_GAP` say how the keys are drawn, which the app and the snapshots share.
- No new crate, crate edge, module, trait, dependency or `unsafe`.

### Colors (defaults now, the theme's later)

M2-23 owns these tokens and checks their contrast. The defaults, in sRGB as the mock-up has them, are
converted to linear constants in `pane.rs`:

| Token | Face | Default |
|---|---|---|
| `float.background` | `Float` | `#474959`, the text area's `#3f3f4a` lifted a little (picked, of `#43444f`, `#474959` and `#4d4f60`) |
| `float.hairline` | `FloatBorder` | white at a low alpha, 3.5% in linear light, so about the mock-up's 9% in sRGB |
| `float.shadow` | `PanelStyle::shadow` | `#08090e` at 55% (picked, of 35%, 55% and 70%) |
| `float.shadow.size` | `PanelStyle` | 10 points down, sigma 14 |
| `float.radius`, `float.hairline.width` | `PanelStyle` | 8 and 1 points |
| `float.label` | `Prompt` | `#bfc7df`, 5.3:1 on the fill, a cool tint apart from the text's (the mock-up's `#9c9eb0` was 3.4:1) |
| `float.field.background` | `Field`'s background | the text area's |
| `float.field.outline` | `Field` | `#6f86bd` |
| `float.key.background` | `Key` | `#55576a` (picked): the mock-up's `#4d4f60` all but vanished on the fill in the app |
| `float.key.foreground` | `Key` | `#dee3f1`, 5.6:1 on its chip |

A question's label keeps the text's color, a prompt's label is dim, as in the mock-up. The keys'
verbs take the label's dim face.

**The theme must be able to switch at run time** (ADR 0027's one-key dark and light, M2-23), so the
float adds no work there:

- every color goes through a `Face`, or through `PanelStyle`, a table in `pane.rs` read every frame:
  the fill, the hairline, the shadow's color and strength, the field, its outline and the chips;
- the shadow's size and strength and the hairline are the theme's values, not fixed math, so a light
  theme can have a lighter, smaller shadow and a dark hairline;
- neither the shaders nor `shape.rs` hold a color. The rounded boxes are built again every frame from
  the faces and the table, and nothing caches an instance, so no color outlives a switch.

### Tests

- Scene tests: the panel is one rounded box in `Layer::FLOAT`, its quad cut to its region; its top on
  the frame's top row reaches the surface's edge; a field keeps to its room; a box out of the region is
  dropped; no float, no rounded box.
- Pixel tests (`just pixel-tests`): a bottom corner leaves the text's background outside its radius;
  the shadow darkens below the panel and fades; nothing of it passes the region's bottom; the field's
  outline and fill; a chip under its letter.
- `oso-core`: the rows of `float_regions` and `minibuffer::layout` at every `t` above, and the keys
  always whole and never under the label or a message, also with odd file names in a question,
  from frames of no rows up. The app draws a float in frames of 0 to 3 rows.
- Text-grid snapshots change: the float starts on row 0, the label has its own row, and the keys show
  as `[y] exit  [n] stay`.

## Where this differs from accepted ADRs

- [ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md): the float is attached to the top
  edge, not one row below it; it has a panel, a field and chips instead of a border and one cell of
  padding; the label has rows of its own; a question shows its keys as chips instead of `(y or n)`;
  small frames give up the padding in the order above, and a label too tall keeps its first rows, not
  its last; saving over a changed file opens with what it asks and the name.
- [ADR 0061](0061-panes-clipping-and-layers.md): a pane in front on the top row covers the margin above
  it where its panel is, and the panel's shadow and field draw outside the float's cells, inside the
  pane's region. Rounded boxes are drawn in a layer between its rectangles and its glyphs, their quads
  cut on the CPU as everything else is.
- [ADR 0008](0008-floating-ui.md) stays as it is: the float is still a buffer in a floating window.
- `ui.md` and `display-model.md` describe the new look.

## Consequences

- The float reads as a panel over the text, as in VS Code, and the text beneath stays in view under
  its shadow.
- Row 0 of the text is hidden while a prompt is open; ADR 0072 kept it.
- A float is one row taller than ADR 0072's for a prompt that reads (4 rows from the top against 3 below
  row 0), and needs 6 frame rows for its full look.
- The field and the chips are off the grid by a few pixels only, so nothing that maps cells to pixels
  changes.
- oso-render gains a pipeline that M3's menus, M2-17's selected candidate and hover documentation can use
  for their own rounded boxes.
- Every question names its verbs, which the review of a new question checks.

## Alternatives considered

- **Content off the grid, with a pixel offset per pane:** exact padding as in the mock-up at every
  scale, but the IME's cursor area, the snapshots and later mouse hits would each need the offset, and
  `Pane` a field every pane sets.
- **The panel's bottom edge in pixels, not on a cell edge:** padding even with the top, but the row under
  it shows a strip of glyph bottoms below the panel.
- **No gap row, the field right under the label:** one row shorter, but the field is the cell's 16 px
  and the label's descenders touch its outline.
- **Flat rectangles only, with corners from small steps:** no new pipeline, but stair-stepped corners and
  no soft shadow.
- **A blur pass for the shadow:** any shape's shadow, at the cost of an offscreen target and a second
  pass every frame a float is open.
- **The chips at the end of the label's last row:** a row shorter for a question, but the label loses
  their width, and a question and a prompt differ in height.
- **Chips with no verbs, or `(y or n)` kept after the chips:** nothing for each question to name, but
  the keys say less, or twice.
- **A hairline of 1 physical pixel at scale 2:** finer, but on a fill this close to the text's it all but
  vanishes, and the shadow does the rest.
- **Keeping the row above the float (ADR 0072):** row 0 stays in view, but the panel then floats
  instead of hanging from the edge, which is what the author chose.
