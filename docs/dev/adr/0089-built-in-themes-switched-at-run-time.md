# 0089. Built-in themes, switched at run time

- Status: Proposed
- Date: 2026-11-08

## Context

[ADR 0027](0027-theme.md) gives oso its own dark and light themes, with a palette of named
colours under semantic mappings, a contrast check in CI and one key to toggle between them. The
colours are still constants behind `Face` in `oso-render`'s `pane.rs`, each step adding its own
(the float of [ADR 0075](0075-a-float-as-a-panel-attached-to-the-top-edge.md), the region of
[ADR 0071](0071-mark-and-region.md), the completion list of
[ADR 0076](0076-completion-and-m-x.md), isearch's matches of [ADR 0082](0082-isearch.md)), all
waiting for M2-23 to make them a theme.

What changed since ADR 0027:

- oso will ship **several themes** made by its author and switch among them. Whether users
  can define their own is not decided, and ADR 0027's TOML overrides are not needed now.
- Configuration arrives in M3, so the choice must be remembered some other way until then.
- The steps since then check their colours against 4.5:1, which they call ADR 0027's floor, while
  ADR 0027 says 7:1 everywhere. Some of them, such as a float's dim label (5.3:1) and isearch's
  current match (4.6:1), fail 7:1. The two isearch fills read as too close together.
- Text is blended in linear space, so light text on a dark background looks heavier than in
  native macOS apps ([ADR 0036](0036-text-system-and-glyph-atlas.md)).
  [ADR 0051](0051-m2-plan-decisions.md) corrects text gamma with the theme, and gives
  neighbouring boxes of invisible characters a 1-pixel gap. Both are part of this step.

What a switch costs today: shaped rows keep a `Face`, not a colour. Glyph and rectangle
instances take their colours from `Face` each frame, rounded boxes are built again each frame,
and glyph rasters are coverage with no colour in them (CoreText's font smoothing is off).

The author chose these on 2026-11-08, through the coordinator.

## Decision

### Three themes, as data

- oso ships three themes of its own, each either dark or light:
  - `dark`, the default: a near-black background and a high-contrast foreground;
  - `light`: its pair, on a light background;
  - `dusk`: the dark grey of the M2 steps so far (`#3f3f4a`), with the float's colours as
    ADR 0075 picked them, so the look tuned on it stays available.
- More themes are more data. Themes stay data so that user themes or overrides remain possible
  later. Variants for colour vision deficiencies are still left for later (ADR 0027).
- No palette is copied from another theme (ADR 0027).

### Where the table lives

- A new public module, `oso-render::theme`, holds `Theme` and `themes()`, the list of built-in
  themes:
  - a theme's name, and whether it is dark or light;
  - its palette of named colours, written in the source as sRGB hex and converted to linear
    once, when the renderer is made, which keeps the list: there is no global state
    ([ADR 0038](0038-architecture-guardrails.md));
  - its face map, from each face's token to palette names, for the foreground and the
    background: ADR 0027's two layers;
  - its `PanelStyle` (ADR 0075's sizes, shadow, hairline and border), which moves into the theme;
  - its text gamma (below).
- The themes are Rust constants. Nothing is parsed at startup.
- The renderer asks the theme for a face's colours (`theme.foreground(face)`,
  `theme.background(face)`), so `pane` does not depend on `theme`. Neither the shaders nor
  shaped rows hold a colour, as ADR 0075 already requires.
- **The choice** is editor state: `oso-core` keeps the themes' names, the current one and the
  toggle's pair in a field of `Editor`. The tip crate fills in the names at startup from
  `themes()`. Commands stay plain functions over `Editor`, and `oso-core` holds no colour.
  The app draws each frame with the current theme.

### Faces

Every theme maps every face. The tokens use the dotted names of ADR 0075's table, which a later
file format would also use:

| Token | Face | What it is |
|---|---|---|
| `text` | `Text` | buffer text and the echo area, on the background behind everything |
| `cursor` | `Cursor` | the cursor box, and the glyphs on it |
| `region` | `Region` | the region's fill (ADR 0071) |
| `search.match`, `search.current` | `SearchMatch`, `SearchCurrent` | isearch's fills (ADR 0082) |
| `escape` | `Escape` | an escape's label (`^M`, `\xFF`) |
| `boxed` | `Boxed` | a boxed code point's label and its box |
| `preedit` | `Preedit` | text an input method is composing, and its underline |
| `warning` | `Warning` | a warning that must be seen: the modeline's and the echo area's, such as `[changed on disk]` |
| `modeline` | `Modeline` | a modeline and its text |
| `agent.on`, `agent.connected` | `AgentOn`, `AgentConnected` | the `Agent` segment, dim and bright ([ADR 0085](0085-agent-channel-and-oso-mcp.md)) |
| `float` | `Float` | a float's fill and text |
| `float.border`, `float.shadow` | `FloatBorder`, `PanelStyle` | its hairline and shadow |
| `prompt` | `Prompt` | a prompt's dim label, and the keys' verbs |
| `question` | `Question` | a question's label (new) |
| `field`, `key` | `Field`, `Key` | an input field and its outline, a key's chip and its letter |
| `completion.border` | `PanelStyle` | a completion float's border |
| `completion.selected` | `Selected` | the selected candidate's bar |
| `completion.match` | `CompletionMatch` | what the input matched in a candidate |
| `completion.annotation`, `completion.binding`, `completion.count` | `Annotation`, `Binding`, `ListPrompt` | what is said of a candidate, its key binding, and the count and label |

- **A question's words never look like buffer text.** A question's label takes `question`, a
  foreground that, in that theme, no face of buffer text uses. ADR 0075 gave it the
  text's colour.
- A completion keeps one match face for every component. A face per component waits until it is
  needed.
- Syntax faces join the table, and its contrast rules, when syntax highlighting comes (M3).

### Contrast, in CI for every theme

A test in `oso-render` checks every theme in `themes()`. Each face declares its pairs in a
`match` with no wildcard, so that a new face does not compile until it has them. Ratios are
WCAG's, from linear colours; a colour with alpha is first laid over what it sits on.

- **7:1 for text that is read:** `text` on the background, and over `region`,
  `search.match`, `search.current` and `completion.selected`; the glyph on the cursor; the
  modeline's text; a float's text and `question` on its fill; `warning` on what it is drawn on;
  `completion.match` on the list's fill and on the selected bar.
- **4.5:1 for secondary labels:** `prompt` on a float's fill, a key's letter on its chip,
  `escape` and `boxed` labels on their backgrounds, `completion.annotation`, `.binding` and
  `.count` on the fill and on the selected bar, and `agent.on` on the modeline.
- **Fills stand apart from what they sit on:** `region`, the two search fills,
  `completion.selected`, the modeline, a float's fill, a key's chip, a field and a box differ
  from the background behind them by a minimum step in OKLab lightness.
- **Faces that must be told apart are told apart:** `search.current` from `search.match`,
  `search.match` from `region`, `question` from `text`, `agent.connected` from `agent.on`, and
  `completion.match` from `text`, by a minimum distance in OKLab.
- The minimum step and distance are set when the colours are picked, from the screenshots the
  author chooses among, and written in the test.

### Text gamma

- The text shader corrects a glyph's coverage by its foreground's luminance, with a gamma and
  a contrast term, as other GPU text renderers do to match the OS's text. Light text on dark
  gets lighter, dark text on light gets heavier, and both match native macOS apps. Blending
  stays linear.
- Each theme has its own terms, in the text shader's uniform. Colour emoji are not changed.
- The terms are tuned against native macOS text in screenshots side by side, and a pixel test
  keeps the curve.
- Rasters stay coverage with no colour, so a switch rasterizes nothing again.

### A switch costs one frame

- A switch changes the current theme and asks for a frame. That frame builds its instances
  from the new theme and writes the text uniform. Nothing is shaped or rasterized again, and no
  cache is cleared. Typing never waits for it.
- A test switches the theme and checks that no row was shaped and no glyph rasterized.

### Commands and the key

- **`load-theme`** reads a theme's name through a completion
  ([ADR 0076](0076-completion-and-m-x.md)), with the current one shown, and switches to it.
- **`toggle-theme`**, on `<f5>`, switches between the two themes of the pair. From a theme
  outside the pair, it goes to the pair's first.
- The pair is `dark` and `light` until M3's configuration can name it.
- Both are `Reach::Person`: the theme is the person's own, and it is remembered on disk.

### Remembering the choice

- The current theme is written to `oso/theme` in the per-user state directory
  (`oso-platform`'s `privacy`), one line with its name, on a worker after a switch. The write
  is whole or nothing.
- `main` reads it before the window exists, at most 256 bytes, through the state directory's
  checks, so the UI thread never waits on it and the first frame is drawn in the right theme. A
  missing file, an unknown name or anything else gives `dark`, with a log line.
- An `oso dev` restart carries the choice in its handoff.
- With M3's configuration, the configuration names the default theme and the pair. The last
  choice still wins until the configured theme changes.

### The OS's appearance

- oso does not follow the OS's dark or light appearance. The person switches with `<f5>` or
  `load-theme`. Following it, as an opt-in entry that picks the pair's dark or light theme, is
  an open question.

### Gaps between boxes of invisible characters

- Where a box of an invisible character is followed by another box, it ends 1 pixel short, so
  that two boxes never read as one (ADR 0051).
- A text-grid snapshot marks a box's last cell with `]`, in place of `#`.

### How colours are chosen

- A sample frame for each theme is drawn off screen, as the pixel tests draw, and written as
  PNGs. It shows text with escapes and boxes, the region, both search fills, a completion float,
  a question, a modeline with the `Agent` segment and a warning in the echo area.
- The author sees them side by side with each pair's ratio, chooses among two or three
  candidates where a colour is open, and the choice is checked once more in the running app.
- `dusk` starts from today's constants. Where they fall short of the floors above, the region
  under text, the modeline's text and its warning, and a box's label, the author picks the
  nearest colours that pass.

## Changes to accepted ADRs

**ADR 0027** (Theme):

- "oso ships its **own** dark and light themes" becomes: oso ships its own themes, three to
  start (`dark`, `light` and `dusk`), each dark or light.
- "Users customize themes by overriding palette entries or mappings in TOML" is no longer a
  requirement. Themes stay data, so user themes and overrides remain possible. Whether they come
  is an open question.
- "A test computes the WCAG contrast ratio of every intended foreground and background pair and
  fails below 7:1" becomes the tiers above: 7:1 for text that is read, 4.5:1 for secondary
  labels, and minimum distances in OKLab for fills and for faces that must be told apart.
- "One key toggles between dark and light. The default is dark." becomes: `<f5>` toggles
  between the pair, `dark` and `light` until configuration names it, and `load-theme` picks any
  theme. The default is `dark`, and the last choice is remembered.

**ADR 0036** (Text system and glyph atlas): its consequence "Text is blended in linear space on
the sRGB surface, so light text on a dark background looks slightly heavier than in native macOS
apps. Revisit with the theme" now holds: the text shader corrects coverage by the foreground's
luminance, with each theme's terms. Blending stays linear.

**ADR 0045** (Text-grid snapshots): its table of marks gains `]`, a boxed invisible character's
last cell.

**ADR 0075** (A float as a panel attached to the top edge):

- Its "Colors (defaults now, the theme's later)" table becomes `dusk`'s values. The tokens are
  every theme's, and `float.label` is `prompt`.
- "A question's label keeps the text's color" becomes: a question's label takes `question`.

ADR 0076 is still Proposed. If it is accepted before this one, its "ADR 0027's 7:1 is the aim,
which M2-23 enforces" changes too: `completion.match` is checked at 7:1, and the annotation,
binding and count at 4.5:1.

## Consequences

- A switch costs one frame and stalls nothing, and a new theme is data.
- Contrast holds in every theme, and a new face cannot skip the check.
- Picking `dark`'s and `light`'s palettes takes rounds of screenshots with the author. Meeting
  7:1 for every text that is read may push the light theme's accents darker than the dark
  theme's.
- The code to come, after M2-17 and M2-20 merge:
  - `pane.rs` loses its constants and `PANEL`;
  - `scene`, `text`, `paint` and the text shader take the theme;
  - the faces are renamed and added as in the table;
  - `theme` joins `oso-render`'s layers in `guardrails/layers.txt` and the public API snapshot;
  - `oso-core` gains the choice on `Editor`, the tip crate the two commands, the state file's
    worker and the handoff field.
- No new crate, crate edge or dependency. No `unsafe`.

## Alternatives considered

- **Two themes, dark and light:** less to pick, but it drops the grey look the M2 floats were
  tuned on.
- **A fourth theme for colour vision deficiencies now:** wanted, but it can be one more set of
  data later.
- **The theme's data in `oso-core`,** for M3's configuration to fill: `oso-core` would hold
  colours it never draws, and more public API.
- **7:1 everywhere, as ADR 0027 says:** dim labels and accents would have to be brighter than
  the author's picks, and the float's label and isearch's current match fail it today.
- **4.5:1 everywhere:** text that is read would be allowed to be harder to read.
- **Rasterizing per theme,** with CoreText's smoothing or a dilation: the atlas would depend on
  the theme, so a switch would clear it and rasterize the visible glyphs again, a hitch.
- **Following the OS's appearance, now or by default:** the author switches by key, and it
  needs appearance events on both window layers.
- **The toggle on a `C-c` key, or unbound:** `C-c` and a letter are left to users in Emacs, and
  `<f5>` is free.
- **The pair as the last two themes loaded, or set with `C-u load-theme`:** more to explain
  before configuration exists.
- **Not remembering the choice until M3:** every start would be `dark`. **macOS user
  defaults:** macOS only, and more native code.
- **Only screenshots in the running app for the colours:** slower to compare, and nothing
  shows the ratios.
- **Keeping a question's label in the text's colour:** the label would share its colour with
  buffer text.
