//! Writing over a card is a compare-and-set.
//!
//! Every other field on a shape takes the last ordered value and loses nothing
//! that was not a coordinate. Words are not like that, so `Change::Text` names
//! the revision it was written against and the reducer refuses when the card
//! has moved on under it — with a token to branch on and the card's current
//! text, verbatim, as the sentence.

use boards_wire::{Board, Change, Shape, TARGET_GONE};
use refusal_class::STALE;

fn one_card() -> Board {
    let board = Board::new("Wall".to_owned(), "ana".to_owned()).unwrap();
    board
        .changed(&Change::Create {
            id: "card".to_owned(),
            shape: Shape::default(),
        })
        .unwrap()
}

fn write(board: &Board, text: &str, base_revision: u64) -> Result<Board, boards_wire::Refused> {
    board.changed(&Change::Text {
        id: "card".to_owned(),
        text: text.to_owned(),
        base_revision,
    })
}

#[test]
fn a_write_against_the_card_as_it_stands_lands_and_moves_it_on() {
    let board = one_card();
    let base = board.shapes["card"].revision;

    let board = write(&board, "the quarter's three bets", base).unwrap();

    assert_eq!(board.shapes["card"].shape.text, "the quarter's three bets");
    assert!(
        board.shapes["card"].revision > base,
        "the write is the next revision, so the next writer has to name it"
    );
}

#[test]
fn a_write_against_a_revision_someone_else_has_passed_is_refused_with_their_words() {
    let board = one_card();
    let base = board.shapes["card"].revision;
    let board = write(&board, "theirs", base).unwrap();

    let refused = write(&board, "mine", base).unwrap_err();

    assert_eq!(refused.reason, STALE);
    assert_eq!(
        refused.sentence, "theirs",
        "the sentence is the card's current text verbatim — no prose around it \
         for the view to peel off before it can show the words"
    );
    assert_eq!(
        board.shapes["card"].shape.text, "theirs",
        "a refused write changes nothing"
    );
}

#[test]
fn a_write_to_a_card_that_is_gone_is_refused_rather_than_swallowed() {
    let board = one_card();
    let base = board.shapes["card"].revision;
    let board = board
        .changed(&Change::Delete {
            id: "card".to_owned(),
        })
        .unwrap();

    let refused = write(&board, "mine", base).unwrap_err();

    assert_eq!(refused.reason, TARGET_GONE);
}

#[test]
fn an_edit_to_another_shape_does_not_stale_this_one() {
    let board = one_card();
    let board = board
        .changed(&Change::Create {
            id: "other".to_owned(),
            shape: Shape::default(),
        })
        .unwrap();
    let base = board.shapes["card"].revision;
    let board = board
        .changed(&Change::Move {
            id: "other".to_owned(),
            x: 400,
            y: 0,
        })
        .unwrap();

    assert!(
        board.revision > base,
        "the board has moved on even though the card has not"
    );
    write(&board, "mine", base).expect("the precondition is the card's revision, not the board's");
}
