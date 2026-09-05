//! 2449's whole contract: the two-region screen, and the sidebar collapsing
//! honestly -- by `ServedBy::is_known()` alone -- when Glasshouse never told
//! pane anything. Every test renders into a `TestBackend` buffer; none opens
//! a terminal.

use pane::contract::{Conversation, Message, Role, ServedBy};
use pane::tui::render;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

fn conversation(messages: Vec<Message>) -> Conversation {
    Conversation {
        system: String::new(),
        messages,
    }
}

fn known_served_by() -> ServedBy {
    ServedBy {
        provider: Some("anthropic".to_string()),
        model: Some("claude-sonnet-5".to_string()),
        route: None,
        quota_context: Some("pro-plan".to_string()),
        input_tokens: Some(123),
        output_tokens: Some(456),
        cached_input_tokens: None,
    }
}

/// Renders `conversation` and `served_by` into an 80x20 buffer.
fn rendered(conversation: &Conversation, served_by: &ServedBy) -> Buffer {
    let backend = TestBackend::new(80, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| render(frame, conversation, served_by))
        .unwrap();
    terminal.backend().buffer().clone()
}

/// Every cell's symbol, concatenated row by row with a newline between rows
/// -- a plain string a test can search without caring where a wrap or a
/// border fell.
fn buffer_text(buffer: &Buffer) -> String {
    let mut text = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            if let Some(cell) = buffer.cell((x, y)) {
                text.push_str(cell.symbol());
            }
        }
        text.push('\n');
    }
    text
}

/// The column each cell in row `y` holds, as one symbol per column -- used
/// to find where a border fell rather than to read prose.
fn row_symbols(buffer: &Buffer, y: u16) -> Vec<String> {
    (0..buffer.area.width)
        .map(|x| {
            buffer
                .cell((x, y))
                .map(|cell| cell.symbol().to_string())
                .unwrap_or_default()
        })
        .collect()
}

/// The x of the sidebar's left border on the top row: the second box-drawing
/// top-left corner from the left, since the conversation column's own border
/// occupies the first.
fn sidebar_left_edge(buffer: &Buffer) -> usize {
    row_symbols(buffer, 0)
        .iter()
        .enumerate()
        .filter(|(_, symbol)| symbol.as_str() == "┌")
        .nth(1)
        .map(|(x, _)| x)
        .expect("both regions draw a bordered box on the top row")
}

#[test]
fn both_regions_render_with_the_conversation_in_order() {
    let conversation = conversation(vec![
        Message::text(Role::User, "hello there"),
        Message::text(Role::Assistant, "general kenobi"),
    ]);
    let text = buffer_text(&rendered(&conversation, &known_served_by()));

    let user_at = text.find("hello there").expect("user message renders");
    let assistant_at = text
        .find("general kenobi")
        .expect("assistant message renders");
    assert!(
        user_at < assistant_at,
        "messages must render in conversation order"
    );
}

#[test]
fn the_sidebar_shows_the_entitlement_that_served_the_last_request() {
    let conversation = conversation(vec![Message::text(Role::User, "hi")]);
    let text = buffer_text(&rendered(&conversation, &known_served_by()));
    assert!(
        text.contains("pro-plan"),
        "the quota_context that served the request must be on screen:\n{text}"
    );
}

#[test]
fn the_sidebar_collapses_rather_than_showing_a_zero() {
    let conversation = conversation(vec![Message::text(Role::User, "hi")]);
    let text = buffer_text(&rendered(&conversation, &ServedBy::default()));

    assert!(
        !text.contains('0'),
        "an unknown ServedBy must render no token figure or other zero:\n{text}"
    );
    assert!(
        text.contains("Glasshouse not connected"),
        "the collapsed sidebar must say plainly that Glasshouse is not connected:\n{text}"
    );
}

#[test]
fn the_collapsed_sidebar_gives_its_width_to_the_conversation() {
    let conversation = conversation(vec![Message::text(Role::User, "hi")]);

    let known = rendered(&conversation, &known_served_by());
    let collapsed = rendered(&conversation, &ServedBy::default());

    let known_edge = sidebar_left_edge(&known);
    let collapsed_edge = sidebar_left_edge(&collapsed);

    assert!(
        collapsed_edge > known_edge,
        "the collapsed sidebar ({collapsed_edge}) must sit further right than \
         the expanded one ({known_edge}), giving the conversation column the width back"
    );
}

#[test]
fn a_message_containing_terminal_escapes_cannot_repaint_the_screen() {
    let hostile = "\x1b[2J\x1b[H\x1b[?1049hpwned";
    let conversation = conversation(vec![Message::text(Role::User, hostile)]);

    let buffer = rendered(&conversation, &known_served_by());

    // The escape bytes are rendered as literal, inert content rather than
    // being interpreted -- because `render` never writes a byte to a
    // terminal itself, it only ever hands ratatui's own widgets a `String`.
    // The proof that nothing repainted the screen is that the sidebar,
    // written after the hostile message in the same frame, still renders
    // exactly as it would without it.
    let text = buffer_text(&buffer);
    assert!(
        text.contains("pwned"),
        "the hostile message's own text still renders:\n{text}"
    );
    assert!(
        text.contains("pro-plan"),
        "a message's escape sequences must not be able to blank or move the \
         sidebar drawn in the same frame:\n{text}"
    );
    assert_eq!(buffer.area.width, 80);
    assert_eq!(buffer.area.height, 20);
}

/// `is_known()` must be the predicate, not a stand-in for it.
///
/// The readout's rows are independently nullable, so a request can be metered
/// — tokens counted — while carrying no provider name. Measured: replacing
/// `served_by.is_known()` with `served_by.provider.is_some()` SURVIVED the
/// whole suite, because every other fixture here either fills both or fills
/// neither. Under that substitution this request renders "Glasshouse not
/// connected", which is the honesty failure 2449 forbids, inverted: it tells
/// the reader nothing was connected when in fact something was and it cost
/// tokens.
#[test]
fn a_metered_request_with_no_provider_name_is_still_known() {
    let served_by = ServedBy {
        provider: None,
        model: None,
        route: None,
        quota_context: None,
        input_tokens: Some(123),
        output_tokens: Some(456),
        cached_input_tokens: None,
    };
    assert!(
        served_by.is_known(),
        "the frozen type already says this is known"
    );

    let conversation = conversation(vec![Message::text(Role::User, "hello")]);
    let text = buffer_text(&rendered(&conversation, &served_by));

    assert!(
        !text.contains("Glasshouse not connected"),
        "a metered request rendered as not connected:\n{text}"
    );
    assert!(
        text.contains("123") && text.contains("456"),
        "the tokens that were counted are missing:\n{text}"
    );
}
