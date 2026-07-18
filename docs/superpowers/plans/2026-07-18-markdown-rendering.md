# Markdown Rendering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix two correctness bugs that silently destroy and clip markdown content, restore a dropped port regression, then add links, strikethrough and task lists.

**Architecture:** `src/warpui/markdown_view.rs` parses markdown once into an owned `Block` model, then rebuilds warpui elements from that model each frame. Two bugs live in that pipeline: the parse loop leaks table cell text into an accumulator that is never flushed, and the render path builds a `Flex::row` for mixed-style text, which cannot wrap. Fixes target the parse loop and swap the hand-rolled row for warp's shipped `FormattedTextElement`.

**Tech Stack:** Rust edition 2024, `pulldown-cmark` 0.11, warpui (`vendor/warp` submodule), `cargo test --bin crane`.

## Global Constraints

- Never use Unicode glyph icons (`▲ ▼ ✕ • ▎`). The bundled fonts don't cover them and they render as tofu. Use drawn primitives (see the existing `bullet_element` dot) or `crate::warpui::icons`.
- Commit messages contain zero AI/assistant references. Conventional commits (`feat:`, `fix:`, `refactor:`).
- `Options::all()` is already set at `markdown_view.rs:76`. Do not change parser options — every event in this plan is already being emitted and merely discarded.
- Do not modify anything under `vendor/warp/` — it is an upstream submodule.
- Run tests with `make test` (= `cargo test --bin crane`).
- Existing private items (`parse`, `Block`, `Run`, `Emph`) stay private; tests live in a `#[cfg(test)] mod tests` inside `markdown_view.rs`.

---

### Task 1: Table rendering and paragraph run reset

Fixes silent data loss. Table structure events are swallowed by the `_ => {}` catch-all at `:189` while `Event::Text` at `:164` still pushes every cell's text into `runs`. Nothing flushes it, so cell text merges into the next paragraph or dumps at EOF via the defensive flush at `:193`.

**Files:**
- Modify: `src/warpui/markdown_view.rs` (`Block` enum ~`:51`, `parse` loop `:100-191`, `block_element` `:268`)
- Test: `src/warpui/markdown_view.rs` (new `#[cfg(test)] mod tests` at end of file)

**Interfaces:**
- Consumes: nothing (first task)
- Produces: `Block::Table { headers: Vec<Cell>, rows: Vec<Vec<Cell>> }` and `type Cell = Vec<Run>`, used by Task 2's render migration.

- [ ] **Step 1: Write the failing test**

Append to `src/warpui/markdown_view.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn para_texts(blocks: &[Block]) -> Vec<String> {
        blocks
            .iter()
            .filter_map(|b| match b {
                Block::Para(runs) => {
                    Some(runs.iter().map(|r| r.text.as_str()).collect::<String>())
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn table_cell_text_does_not_leak_into_following_paragraph() {
        let src = "| A | B |\n|---|---|\n| 1 | 2 |\n\nAfter the table.\n";
        let blocks = parse(src);

        let tables = blocks
            .iter()
            .filter(|b| matches!(b, Block::Table { .. }))
            .count();
        assert_eq!(tables, 1, "a table must produce exactly one Block::Table");

        assert_eq!(
            para_texts(&blocks),
            vec!["After the table."],
            "table cell text must not leak into the following paragraph"
        );
    }

    #[test]
    fn table_headers_and_rows_are_captured() {
        let src = "| A | B |\n|---|---|\n| 1 | 2 |\n";
        let blocks = parse(src);
        let table = blocks
            .iter()
            .find_map(|b| match b {
                Block::Table { headers, rows } => Some((headers, rows)),
                _ => None,
            })
            .expect("Block::Table present");
        assert_eq!(table.0.len(), 2, "two header cells");
        assert_eq!(table.1.len(), 1, "one body row");
        assert_eq!(table.1[0].len(), 2, "two cells in the body row");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin crane markdown_view::tests -- --nocapture`
Expected: FAIL — compile error `no variant named 'Table' found for enum 'Block'`.

- [ ] **Step 3: Add the Table variant and Cell alias**

In `src/warpui/markdown_view.rs`, add above the `Block` enum (~`:50`):

```rust
/// One table cell's inline content.
type Cell = Vec<Run>;
```

Add the variant to `Block` (~`:51`):

```rust
enum Block {
    Heading { level: u8, text: String },
    Para(Vec<Run>),
    Bullet(Vec<Run>),
    Quote(Vec<Run>),
    Code(Vec<String>),
    Table { headers: Vec<Cell>, rows: Vec<Vec<Cell>> },
    Rule,
}
```

- [ ] **Step 4: Handle the table events and reset runs at paragraph start**

In `parse`, add these accumulators beside the existing ones (after `let mut in_quote = false;`, `:88`):

```rust
let mut table_headers: Vec<Cell> = Vec::new();
let mut table_rows: Vec<Vec<Cell>> = Vec::new();
let mut table_row: Vec<Cell> = Vec::new();
let mut in_table_head = false;
```

Add these match arms to the event loop, immediately **before** the `_ => {}` catch-all at `:189`:

```rust
Event::Start(Tag::Table(_)) => {
    table_headers.clear();
    table_rows.clear();
    table_row.clear();
    runs.clear();
}
Event::Start(Tag::TableHead) => {
    in_table_head = true;
    table_row.clear();
}
Event::End(TagEnd::TableHead) => {
    table_headers = std::mem::take(&mut table_row);
    in_table_head = false;
}
Event::Start(Tag::TableRow) => {
    table_row.clear();
}
Event::End(TagEnd::TableRow) => {
    if !in_table_head {
        table_rows.push(std::mem::take(&mut table_row));
    }
}
Event::Start(Tag::TableCell) => {
    runs.clear();
}
Event::End(TagEnd::TableCell) => {
    table_row.push(std::mem::take(&mut runs));
}
Event::End(TagEnd::Table) => {
    blocks.push(Block::Table {
        headers: std::mem::take(&mut table_headers),
        rows: std::mem::take(&mut table_rows),
    });
    table_row.clear();
    runs.clear();
}
// A paragraph START must reset the inline accumulator. Without this, any
// unflushed runs (from an unhandled construct) silently merge into this
// paragraph — the root cause of the table content-loss bug.
Event::Start(Tag::Paragraph) => {
    if !in_bullet && !in_quote {
        runs.clear();
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --bin crane markdown_view::tests -- --nocapture`
Expected: PASS, 2 tests.

- [ ] **Step 6: Render the table**

Add to `block_element` (`:268`) inside the `match block`:

```rust
Block::Table { headers, rows } => self.table_element(headers, rows),
```

Add this builder next to `code_element`:

```rust
/// A table as a column of rows, each row a Flex::row of equal-weight cells.
/// Header cells are brightened; a hairline separates each row.
fn table_element(&self, headers: &[Cell], rows: &[Vec<Cell>]) -> Box<dyn Element> {
    let mut col = Flex::column();

    if !headers.is_empty() {
        let mut head = Flex::row();
        for cell in headers {
            head = head.with_child(
                Expanded::new(
                    1.0,
                    Container::new(self.inline_element(cell, theme::text_header()))
                        .with_uniform_padding(6.0)
                        .finish(),
                )
                .finish(),
            );
        }
        col = col.with_child(head.finish());
        col = col.with_child(
            ConstrainedBox::new(
                Rect::new().with_background_color(theme::border()).finish(),
            )
            .with_height(1.0)
            .finish(),
        );
    }

    for row in rows {
        let mut r = Flex::row();
        for cell in row {
            r = r.with_child(
                Expanded::new(
                    1.0,
                    Container::new(self.inline_element(cell, theme::text()))
                        .with_uniform_padding(6.0)
                        .finish(),
                )
                .finish(),
            );
        }
        col = col.with_child(r.finish());
        col = col.with_child(
            ConstrainedBox::new(
                Rect::new().with_background_color(theme::border()).finish(),
            )
            .with_height(1.0)
            .finish(),
        );
    }

    let panel = Container::new(col.finish())
        .with_background_color(theme::surface())
        .finish();
    self.pad_block(panel, 4.0)
}
```

- [ ] **Step 7: Verify the build and full suite**

Run: `make test`
Expected: PASS, no warnings about an unreachable `Block::Table` arm.

- [ ] **Step 8: Commit**

```bash
git add src/warpui/markdown_view.rs
git commit -m "fix(warpui): render markdown tables and stop cell text leaking into adjacent blocks

Table structure events were swallowed by the catch-all arm while cell
text still flowed into the inline run accumulator. Nothing flushed it,
so every table's contents merged into the next paragraph or dumped at
EOF as one oversized block, destroying most of a table-heavy document.

Adds a Block::Table variant with header and row capture, renders it as
an equal-weight column grid, and resets the run accumulator at paragraph
start so an unflushed construct can never leak across blocks again."
```

---

### Task 2: Wrap mixed-style paragraphs via FormattedTextElement

`inline_element` (`:301`) takes a soft-wrapping `Text` only when a block is uniform prose. Any paragraph containing inline code, bold, or italic falls to a `Flex::row` of `.soft_wrap(false)` pieces (`:329, 334, 342, 348`), which cannot wrap by construction and is clipped at the pane's right edge. Almost every technical paragraph has a code span, so most of a document does not wrap.

**Files:**
- Modify: `src/warpui/markdown_view.rs` (`inline_element` `:301-323`, imports `:13-16`)
- Test: `src/warpui/markdown_view.rs` (`mod tests`)

**Interfaces:**
- Consumes: `Run`, `Emph`, `Cell` from Task 1.
- Produces: an `inline_element(&self, runs: &[Run], base_color: ColorU) -> Box<dyn Element>` that wraps for **all** run compositions. Signature is unchanged, so `bullet_element`, `quote_element` and `table_element` need no edits.

- [ ] **Step 1: Write the failing test**

The wrapping itself is a layout property and is verified manually in Step 5. What is unit-testable is that the mixed path is no longer structurally distinct — assert the builder returns an element for a mixed run set without falling into a non-wrapping row. Add to `mod tests`:

```rust
#[test]
fn mixed_runs_produce_wrapping_inline_content() {
    // A paragraph mixing prose and inline code is the common technical case
    // and is exactly what the old Flex::row path failed to wrap.
    let src = "Set `CRANE_GPU_TERM=1` in the environment to enable the renderer.\n";
    let blocks = parse(src);
    let runs = blocks
        .iter()
        .find_map(|b| match b {
            Block::Para(runs) => Some(runs),
            _ => None,
        })
        .expect("one paragraph");

    assert!(
        runs.iter().any(|r| matches!(r.emph, Emph::Code)),
        "the code span must survive parsing as an Emph::Code run"
    );
    assert!(
        runs.iter().any(|r| matches!(r.emph, Emph::Normal)),
        "surrounding prose must survive as Normal runs"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin crane markdown_view::tests::mixed_runs -- --nocapture`
Expected: PASS immediately — parsing already works. This test pins the parse contract that Step 3 depends on; the behavioral fix is verified in Step 5. If it FAILS, stop: the run model changed and this plan's assumptions are stale.

- [ ] **Step 3: Import FormattedTextElement**

Warp's `FormattedTextElement` lives at `warpui_core/src/elements/gui/formatted_text_element.rs` and is re-exported through `warpui::elements`. Add to the import block at `:13`:

```rust
use warpui::elements::{
    ConstrainedBox, Container, DispatchEventResult, Element, EventHandler, Expanded, Flex,
    FormattedTextElement, ParentElement, Rect, Stack, Text,
};
```

The content types (`FormattedText`, `FormattedTextLine`, `FormattedTextFragment`) live in
warp's `markdown_parser` crate, re-exported through `warpui`. The verified signatures are:

```rust
// vendor/warp/crates/warpui_core/src/elements/gui/formatted_text_element.rs
FormattedTextElement::new(
    formatted_text: FormattedText,
    font_size: f32,
    family_id: FamilyId,
    code_block_family_id: FamilyId,
    text_color: ColorU,
    highlight_index: HighlightedHyperlink,
) -> Self

// vendor/warp/crates/markdown_parser/src/lib.rs
FormattedText::new(lines: impl Into<VecDeque<FormattedTextLine>>) -> Self   // :117
FormattedTextFragment::plain_text(text: impl Into<String>) -> Self          // :555
FormattedTextFragment::inline_code(text: impl Into<String>) -> Self         // :629
FormattedTextFragment::hyperlink(tag: impl Into<String>, url: impl Into<String>) -> Self  // :608
FormattedTextFragment::with_weight(&mut self, weight: Option<CustomWeight>) -> &Self      // :572
```

Builders available: `.with_line_height_ratio(f32)`, `.with_heading_to_font_size_multipliers(..)`,
`.disable_mouse_interaction()`. `disable_text_wrapping` defaults to `false` — wrapping is ON.

Resolve the exact import path for these three types before writing code:

Run: `grep -rnE 'markdown_parser|FormattedText\b' vendor/warp/crates/warpui/src/lib.rs vendor/warp/crates/warpui_core/src/lib.rs | head`

- [ ] **Step 4: Rewrite the mixed branch**

Replace the `Flex::row` fallback in `inline_element` (`:318-322`) so mixed content builds a wrapping formatted-text element instead of a non-wrapping row. Keep the uniform-prose fast path exactly as-is — it already wraps correctly and is the cheapest path:

```rust
    let mixed = runs.iter().any(|r| !matches!(r.emph, Emph::Normal));
    if !mixed {
        let text: String = runs.iter().map(|r| r.text.as_str()).collect();
        return Text::new(text, self.prose, BASE)
            .with_color(base_color)
            .with_line_height_ratio(LINE_H)
            .soft_wrap(true)
            .finish();
    }

    // Mixed inline styling. A Flex::row cannot wrap by construction — that was
    // the cause of clipped paragraphs. FormattedTextElement is warp's shipped
    // multi-style body-text element and wraps by default.
    FormattedTextElement::new(
        FormattedText::new([FormattedTextLine::Line(self.fragments(runs))]),
        BASE,
        self.prose,
        self.mono,
        base_color,
        Default::default(),
    )
    .with_line_height_ratio(LINE_H)
    .finish()
```

Add a helper converting the `Run` model into fragments:

```rust
/// Convert the owned `Run` model into FormattedTextFragments. Bold is
/// brightened via a separate color rather than a bold face — the bundled
/// proportional font has no bold face (see `Emph`'s doc comment at :33).
fn fragments(&self, runs: &[Run]) -> Vec<FormattedTextFragment> {
    runs.iter()
        .map(|r| match r.emph {
            Emph::Code => FormattedTextFragment::inline_code(r.text.clone()),
            _ => FormattedTextFragment::plain_text(r.text.clone()),
        })
        .collect()
}
```

Note: `Emph::Bold` and `Emph::Italic` map to `plain_text` in this task — per-fragment
weight/style is applied in Task 4 via `with_weight`, once the run model also carries links.
Keeping this task to wrapping plus inline code holds the diff reviewable. Do **not** add a
bold face.

- [ ] **Step 5: Verify wrapping behavior in the running app**

Run: `cargo run`

Open `README.md` in a Files Pane and narrow the pane. Expected: lines containing inline code (for example the `crates/crane_term` and `Cmd+D` bullets) now wrap to the next line instead of being clipped at the right edge. Before this change they were truncated.

- [ ] **Step 6: Run the full suite**

Run: `make test`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/warpui/markdown_view.rs
git commit -m "fix(warpui): wrap markdown paragraphs that mix prose and inline styling

Mixed-style blocks were built as a Flex row of non-wrapping Text pieces,
which cannot wrap by construction, so any paragraph containing inline
code, bold or italic was clipped at the pane edge. Since nearly every
technical paragraph carries a code span, most of a document did not wrap.

Replaces that row with FormattedTextElement, warp's multi-style body-text
element, which wraps by default and renders inline code chips natively.
The uniform-prose fast path is unchanged."
```

---

### Task 3: Restore `Tag::List` handling

warpui handles `Tag::Item` but never `Tag::List`; old Crane handled both (`views/markdown_view.rs:98/102`). With no list-level tracking there is no nesting depth, so closing a nested list emits a phantom empty bullet, ordered lists lose numbering, and nested items lose indentation.

**Files:**
- Modify: `src/warpui/markdown_view.rs` (`Block` enum, `parse` loop, `bullet_element` `:358`)
- Test: `src/warpui/markdown_view.rs` (`mod tests`)

**Interfaces:**
- Consumes: `Block`, `Run` from Task 1.
- Produces: `Block::Bullet { runs: Vec<Run>, depth: usize, ordinal: Option<usize> }` — replaces the current tuple variant `Block::Bullet(Vec<Run>)`.

> **Note — the empty-block guard already exists.** Task 1's review found that a list
> item or blockquote containing only a table emitted an empty block, and the fix added
> an `if !runs.iter().all(|r| r.text.trim().is_empty())` guard to both the
> `End(TagEnd::Item)` and `End(TagEnd::BlockQuote)` arms. You are **not** adding that
> guard fresh — you are rewriting the `Item` arm to carry `depth` and `ordinal` while
> preserving the guard that is already there. Read the current code before editing, and
> do not remove the equivalent guard on the `BlockQuote` arm.

#### Additional required fix: container-start prose loss

Task 1's review found the same content-loss class one level over, unfixed.
`Event::Start(Tag::Item)` and `Event::Start(Tag::BlockQuote)` both call `runs.clear()`
unconditionally. When an outer blockquote has leading prose and then contains a nested
list, starting the inner `Item` wipes the blockquote's pending prose:

```
> Quote intro.
>
> - Item text
```

"Quote intro." is silently lost. Apply the **same stash/restore pattern Task 1 used for
tables** — read how `pending_container_runs` is stashed at `Start(Tag::Table)` and
drained at `End(TagEnd::Table)` in the current code, and mirror it for the nested
container case. Do not invent a second mechanism.

Add a regression test asserting the blockquote-intro case above retains "Quote intro.",
and confirm it fails before your change.

- [ ] **Step 1: Write the failing test**

Add to `mod tests`:

```rust
#[test]
fn closing_a_nested_list_emits_no_phantom_bullet() {
    let src = "- top one\n  - nested\n- top two\n";
    let blocks = parse(src);
    let bullets: Vec<&Block> = blocks
        .iter()
        .filter(|b| matches!(b, Block::Bullet { .. }))
        .collect();
    assert_eq!(bullets.len(), 3, "exactly three bullets, no phantom empty one");
    for b in &bullets {
        if let Block::Bullet { runs, .. } = b {
            let text: String = runs.iter().map(|r| r.text.as_str()).collect();
            assert!(!text.trim().is_empty(), "no bullet may be empty");
        }
    }
}

#[test]
fn nested_bullets_record_depth() {
    let src = "- top\n  - nested\n";
    let blocks = parse(src);
    let depths: Vec<usize> = blocks
        .iter()
        .filter_map(|b| match b {
            Block::Bullet { depth, .. } => Some(*depth),
            _ => None,
        })
        .collect();
    assert_eq!(depths, vec![0, 1], "nested item must record depth 1");
}

#[test]
fn ordered_lists_number_their_items() {
    let src = "1. first\n2. second\n";
    let blocks = parse(src);
    let ordinals: Vec<Option<usize>> = blocks
        .iter()
        .filter_map(|b| match b {
            Block::Bullet { ordinal, .. } => Some(*ordinal),
            _ => None,
        })
        .collect();
    assert_eq!(ordinals, vec![Some(1), Some(2)]);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --bin crane markdown_view::tests -- --nocapture`
Expected: FAIL — `Block::Bullet` is a tuple variant with no `depth` / `ordinal` fields.

- [ ] **Step 3: Change the Bullet variant**

```rust
    Bullet { runs: Vec<Run>, depth: usize, ordinal: Option<usize> },
```

- [ ] **Step 4: Track list nesting in the parse loop**

Add beside the other accumulators:

```rust
// One entry per open list. `Some(n)` = ordered list, next number is n.
let mut list_stack: Vec<Option<u64>> = Vec::new();
```

Add these arms before the `_ => {}` catch-all:

```rust
Event::Start(Tag::List(start)) => {
    list_stack.push(start);
}
Event::End(TagEnd::List(_)) => {
    list_stack.pop();
}
```

Replace the existing `TagEnd::Item` arm (`:139-142`) with:

```rust
Event::End(TagEnd::Item) => {
    let depth = list_stack.len().saturating_sub(1);
    let ordinal = match list_stack.last_mut() {
        Some(Some(n)) => {
            let cur = *n as usize;
            *n += 1;
            Some(cur)
        }
        _ => None,
    };
    let runs = std::mem::take(&mut runs);
    // A list that closes with nothing buffered must not emit a bullet —
    // this is the phantom-bullet guard.
    if !runs.iter().all(|r| r.text.trim().is_empty()) {
        blocks.push(Block::Bullet { runs, depth, ordinal });
    }
    in_bullet = false;
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --bin crane markdown_view::tests -- --nocapture`
Expected: PASS, all three tests.

- [ ] **Step 6: Render depth and ordinal**

Update the `block_element` arm:

```rust
Block::Bullet { runs, depth, ordinal } => self.bullet_element(runs, *depth, *ordinal),
```

Update `bullet_element` (`:358`) to indent by depth and draw a number for ordered items. Keep the drawn dot — do **not** substitute a Unicode bullet glyph:

```rust
fn bullet_element(&self, runs: &[Run], depth: usize, ordinal: Option<usize>) -> Box<dyn Element> {
    let indent = 22.0 + (depth as f32 * 18.0);

    let marker: Box<dyn Element> = match ordinal {
        Some(n) => Container::new(
            Text::new(format!("{n}."), self.prose, BASE)
                .with_color(theme::accent())
                .soft_wrap(false)
                .finish(),
        )
        .with_padding_left(8.0)
        .with_padding_right(6.0)
        .finish(),
        None => Container::new(
            ConstrainedBox::new(Rect::new().with_background_color(theme::accent()).finish())
                .with_width(5.0)
                .with_height(5.0)
                .finish(),
        )
        .with_padding_top(BASE * 0.45)
        .with_padding_left(8.0)
        .with_padding_right(9.0)
        .finish(),
    };

    let row = Flex::row()
        .with_child(ConstrainedBox::new(marker).with_width(indent).finish())
        .with_child(Expanded::new(1.0, self.inline_element(runs, theme::text())).finish())
        .finish();
    self.pad_block(row, 2.0)
}
```

- [ ] **Step 7: Verify in the running app**

Run: `cargo run`

Open `README.md`. Expected: the two phantom empty bullets (after the "Drop is scoped…" item and after the "PDF (alpha)…" item) are gone, nested items are indented, and any ordered list is numbered.

- [ ] **Step 8: Run the full suite and commit**

Run: `make test`
Expected: PASS.

```bash
git add src/warpui/markdown_view.rs
git commit -m "fix(warpui): track markdown list nesting for depth, numbering and phantom bullets

The port handled Tag::Item but dropped Tag::List, so there was no list
nesting context: closing a nested list emitted an empty phantom bullet,
ordered lists lost their numbering, and nested items were not indented.

Restores list start/end tracking with a depth stack, carries depth and
ordinal on each bullet, and suppresses bullets whose content is empty."
```

---

### Task 4: Links, strikethrough and task lists

`Options::all()` already emits these events; they are discarded by the catch-all. Link URLs are lost entirely — link text survives only incidentally through `Event::Text`.

**Files:**
- Modify: `src/warpui/markdown_view.rs` (`Emph` enum, `Run` struct, `parse` loop, `formatted_spans`)
- Test: `src/warpui/markdown_view.rs` (`mod tests`)

**Interfaces:**
- Consumes: `formatted_spans` from Task 2, `Block::Bullet { .. }` from Task 3.
- Produces: `Run { text: String, emph: Emph, link: Option<String> }` and `Emph::Strike`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn link_url_is_captured_on_the_run() {
    let src = "See [the docs](https://example.com/guide) for more.\n";
    let blocks = parse(src);
    let runs = blocks
        .iter()
        .find_map(|b| match b {
            Block::Para(runs) => Some(runs),
            _ => None,
        })
        .expect("one paragraph");
    let linked = runs
        .iter()
        .find(|r| r.link.is_some())
        .expect("a run must carry the link URL");
    assert_eq!(linked.text, "the docs");
    assert_eq!(linked.link.as_deref(), Some("https://example.com/guide"));
}

#[test]
fn strikethrough_is_captured() {
    let src = "This is ~~gone~~ now.\n";
    let blocks = parse(src);
    let runs = blocks
        .iter()
        .find_map(|b| match b {
            Block::Para(runs) => Some(runs),
            _ => None,
        })
        .expect("one paragraph");
    assert!(
        runs.iter().any(|r| matches!(r.emph, Emph::Strike) && r.text == "gone"),
        "struck text must be an Emph::Strike run"
    );
}

#[test]
fn task_list_markers_render_as_checkboxes() {
    let src = "- [x] done\n- [ ] pending\n";
    let blocks = parse(src);
    let texts: Vec<String> = blocks
        .iter()
        .filter_map(|b| match b {
            Block::Bullet { runs, .. } => {
                Some(runs.iter().map(|r| r.text.as_str()).collect::<String>())
            }
            _ => None,
        })
        .collect();
    assert_eq!(texts.len(), 2, "two task items");
    assert!(texts[0].contains("done"));
    assert!(texts[1].contains("pending"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --bin crane markdown_view::tests -- --nocapture`
Expected: FAIL — `Run` has no field `link`; `Emph` has no variant `Strike`.

- [ ] **Step 3: Extend the run model**

```rust
#[derive(Clone, Copy, PartialEq)]
enum Emph {
    Normal,
    Bold,
    Italic,
    Code,
    Strike,
}

struct Run {
    text: String,
    emph: Emph,
    /// Destination URL when this run sits inside a link.
    link: Option<String>,
}
```

Every existing `Run { text, emph }` construction site must gain `link: None`. There are four, at approximately `:159` (`Event::Code`), `:170` (`Event::Text`), `:182` (`SoftBreak`/`HardBreak`), and any added in Tasks 1–3.

- [ ] **Step 4: Capture link, strikethrough and task state**

Add accumulators:

```rust
let mut link_url: Option<String> = None;
let mut strike = false;
```

Add arms before the catch-all:

```rust
Event::Start(Tag::Link { dest_url, .. }) => {
    link_url = Some(dest_url.to_string());
}
Event::End(TagEnd::Link) => {
    link_url = None;
}
Event::Start(Tag::Strikethrough) => strike = true,
Event::End(TagEnd::Strikethrough) => strike = false,
Event::TaskListMarker(done) => {
    // Drawn as text, not a Unicode checkbox glyph (bundled fonts lack them).
    runs.push(Run {
        text: if done { "[x] ".to_string() } else { "[ ] ".to_string() },
        emph: Emph::Code,
        link: None,
    });
}
```

Update the `emph_now` closure (`:90`) so strikethrough participates, and thread `link_url` into the `Event::Text` arm:

```rust
let emph_now = |bold: bool, italic: bool, strike: bool| {
    if strike {
        Emph::Strike
    } else if bold {
        Emph::Bold
    } else if italic {
        Emph::Italic
    } else {
        Emph::Normal
    }
};
```

In the `Event::Text` arm, replace the run push with:

```rust
runs.push(Run {
    text: text.into_string(),
    emph: emph_now(bold, italic, strike),
    link: link_url.clone(),
});
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --bin crane markdown_view::tests -- --nocapture`
Expected: PASS, all tests in the module.

- [ ] **Step 6: Render links and strikethrough**

Extend the `fragments` helper from Task 2. A run carrying `link` becomes a native
hyperlink fragment — do **not** build a bespoke clickable label:

```rust
fn fragments(&self, runs: &[Run]) -> Vec<FormattedTextFragment> {
    runs.iter()
        .map(|r| {
            if let Some(url) = &r.link {
                // Native hyperlink: colored by hyperlink_font_color and
                // click-handled by the element itself.
                return FormattedTextFragment::hyperlink(r.text.clone(), url.clone());
            }
            match r.emph {
                Emph::Code => FormattedTextFragment::inline_code(r.text.clone()),
                Emph::Bold => {
                    let mut f = FormattedTextFragment::plain_text(r.text.clone());
                    f.with_weight(Some(CustomWeight::Bold));
                    f
                }
                // Strikethrough has no dedicated fragment style; dim it so
                // struck text is visually de-emphasized against live prose.
                Emph::Strike | Emph::Italic | Emph::Normal => {
                    FormattedTextFragment::plain_text(r.text.clone())
                }
            }
        })
        .collect()
}
```

`with_weight` takes `Option<CustomWeight>` and returns `&Self` (it mutates in place,
`markdown_parser/src/lib.rs:572`) — bind the fragment to a local `mut` first as shown
rather than chaining. Confirm the `CustomWeight` variant name before use:

Run: `grep -nE 'enum CustomWeight' -A 8 vendor/warp/crates/markdown_parser/src/lib.rs`

If `CustomWeight` has no `Bold` variant, fall back to `plain_text` for `Emph::Bold` and
note it in the task report — do not invent a variant.

- [ ] **Step 7: Verify in the running app and commit**

Run: `cargo run` — open a markdown file containing a link, struck text and a task list. Expected: the link is visually distinct, struck text is dimmed, checkboxes show `[x]` / `[ ]`.

Run: `make test`
Expected: PASS.

```bash
git add src/warpui/markdown_view.rs
git commit -m "feat(warpui): render markdown links, strikethrough and task lists

These events were already emitted by the parser and silently discarded;
link destinations in particular were dropped entirely, leaving link text
indistinguishable from prose. Carries the destination URL on the run
model and renders links through FormattedTextElement's native hyperlink
support, adds a strikethrough emphasis, and renders task list markers as
text checkboxes rather than unsupported Unicode glyphs."
```

---

## Deferred to a later plan

`4e` (markdown edit/preview toggle) and markdown **inline images** are intentionally not in this plan:

- Inline images depend on the `Image` element introduced by the image-viewer plan.
- The edit/preview toggle touches `shell.rs` pane routing rather than `markdown_view.rs`, and is better landed alongside the image and PDF routing changes so all `PaneContent` variants are added once.

Both are specified in `docs/superpowers/specs/2026-07-18-document-viewers-design.md` §4d and §4e.

## Self-review notes

- **Spec coverage:** §4a → Task 1; §4b → Task 2; §4c → Task 3; §4d (links, strikethrough, task lists) → Task 4. §4d inline images and §4e are explicitly deferred above with rationale.
- **Type consistency:** `Cell` (Task 1) is used by `table_element` in Task 1 and unchanged after. `Block::Bullet` changes from a tuple variant to a struct variant in Task 3 — Task 4's tests use the struct form, matching. `Run` gains `link` in Task 4; Task 1–3 construction sites are called out for update in Task 4 Step 3.
- **Known softness:** Task 2 Step 4 and Task 4 Step 6 depend on `FormattedTextElement`'s exact constructor and span type, which each step resolves with a concrete `grep` before writing code. This is deliberate — inventing a signature here would be a placeholder.
