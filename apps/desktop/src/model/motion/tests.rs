use std::collections::{BTreeMap, BTreeSet};

use proptest::prelude::*;

use super::*;

fn ms(n: u32) -> Duration {
    millis(n)
}

fn task(n: u8) -> TaskId {
    TaskId::from_uuid(uuid::Uuid::from_bytes([n; 16]))
}

const MY_DAY: RoomKey = RoomKey::Smart(crate::model::Smart::MyDay);

/// Where a full-pace completion starts to leave, and where it's released.
const EXIT_AT: u32 = m::CHECK_TICK_DELAY + m::CHECK_TICK + m::COMPLETE_HOLD;
const RELEASE_AT: u32 = EXIT_AT + m::ROW_COLLAPSE;

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

#[test]
fn a_completion_fills_draws_holds_then_collapses_and_is_released() {
    let id = task(1);
    let mut mo = Motion::default();
    assert_eq!(mo.complete(id, ms(0), Pace::FULL), Keep::Holding);
    assert_eq!(mo.holding().collect::<Vec<_>>(), [id]);

    // The disc starts small and empty, and pops past its size before it settles.
    let start = mo.row(id, ms(0)).expect("moving");
    assert_eq!((start.fill, start.scale), (0., m::CHECK_FILL_FROM_SCALE));
    assert!((1..m::CHECK_FILL).any(|t| mo.row(id, ms(t)).expect("moving").scale > 1.));
    let filled = mo.row(id, ms(m::CHECK_FILL)).expect("moving");
    assert_eq!((filled.fill, filled.scale), (1., 1.));
    // The tick waits for its delay, then draws with the strike-through and the dimming title.
    assert_eq!(
        mo.row(id, ms(m::CHECK_TICK_DELAY)).expect("moving").tick,
        0.
    );
    let mid = mo
        .row(id, ms(m::CHECK_TICK_DELAY + m::CHECK_TICK / 2))
        .expect("moving");
    assert!(mid.tick > 0. && mid.tick < 1. && mid.strike == mid.tick && mid.dim == mid.tick);

    // Holding: done, struck through and still in place, and nothing to draw until it leaves.
    let landed = m::CHECK_TICK_DELAY + m::CHECK_TICK;
    for t in [landed, landed + m::COMPLETE_HOLD / 2, EXIT_AT - 1] {
        assert_eq!(mo.row(id, ms(t)), Some(RowLook::rest(true)), "{t}");
        assert_eq!(mo.wake(ms(t)), Wake::At(ms(EXIT_AT)), "{t}");
    }
    assert!(mo.advance(ms(EXIT_AT)).is_empty());

    // Leaving: fading and collapsing, still held.
    assert_eq!(mo.wake(ms(EXIT_AT)), Wake::Frame);
    let leaving = mo
        .row(id, ms(EXIT_AT + m::ROW_COLLAPSE / 2))
        .expect("moving");
    assert!(leaving.height > 0. && leaving.height < 1.);
    assert!(leaving.opacity > 0. && leaving.opacity < 1.);
    assert!(mo.advance(ms(RELEASE_AT - 1)).is_empty());

    // Released once it has gone; then it enters Completed (if that shows it) and comes to rest.
    assert_eq!(mo.advance(ms(RELEASE_AT)), [Release::Completed(id)]);
    assert_eq!(mo.holding().count(), 0);
    assert_eq!(mo.row(id, ms(RELEASE_AT)).expect("entering").height, 0.);
    assert!(mo.advance(ms(RELEASE_AT + m::ROW_ENTER)).is_empty());
    assert_eq!(mo.row(id, ms(RELEASE_AT + m::ROW_ENTER)), None);
    assert_eq!(mo.wake(ms(RELEASE_AT + m::ROW_ENTER)), Wake::Idle);
}

#[test]
fn reduced_motion_snaps_movement_caps_fades_and_keeps_the_hold() {
    let id = task(1);
    let mut mo = Motion::default();
    mo.complete(id, ms(0), Pace::REDUCED);
    // The checkbox switches at once: no pop, no drawing.
    let first = mo.row(id, ms(0)).expect("moving");
    assert_eq!(
        (first.fill, first.scale, first.tick, first.strike),
        (1., 1., 1., 1.)
    );
    // The title dims in REDUCED_FADE.
    let half = mo.row(id, ms(m::REDUCED_FADE / 2)).expect("moving");
    assert!(half.dim > 0. && half.dim < 1.);
    // The hold keeps its full length, then the row only fades: it never collapses.
    let exit = m::REDUCED_FADE + m::COMPLETE_HOLD;
    assert_eq!(mo.wake(ms(m::REDUCED_FADE)), Wake::At(ms(exit)));
    let fading = mo.row(id, ms(exit + m::REDUCED_FADE / 2)).expect("moving");
    assert_eq!(fading.height, 1.);
    assert!(fading.opacity > 0. && fading.opacity < 1.);
    let release = exit + m::REDUCED_FADE;
    assert!(mo.advance(ms(release - 1)).is_empty());
    assert_eq!(mo.advance(ms(release)), [Release::Completed(id)]);
    // It enters Completed without growing.
    assert_eq!(mo.row(id, ms(release)).expect("entering").height, 1.);
}

#[test]
fn toggling_back_during_the_hold_restores_the_row_in_place_without_a_jump() {
    let id = task(1);
    let mut mo = Motion::default();
    mo.complete(id, ms(0), Pace::FULL);
    let at = ms(400);
    let before = mo.row(id, at).expect("moving");
    assert_eq!(mo.reopen(id, at, Pace::FULL), Keep::InPlace);
    assert_eq!(mo.row(id, at), Some(before));
    assert_eq!(mo.holding().count(), 0);
    assert_eq!(mo.reopening().count(), 0);
    // The disc clears, the strike draws back, and it's never released.
    let mid = mo.row(id, at + ms(m::OVERLAY_OUT / 2)).expect("moving");
    assert!(mid.fill < 1. && mid.fill > 0. && mid.strike < 1.);
    assert!(mo.advance(ms(5_000)).is_empty());
    assert_eq!(mo.row(id, ms(5_000)), None);
    assert_eq!(mo.wake(ms(5_000)), Wake::Idle);
}

#[test]
fn toggling_back_while_collapsing_grows_the_row_back() {
    let id = task(1);
    let mut mo = Motion::default();
    mo.complete(id, ms(0), Pace::FULL);
    let at = ms(EXIT_AT + m::ROW_COLLAPSE / 2);
    let before = mo.row(id, at).expect("moving");
    assert!(before.height < 1.);
    mo.reopen(id, at, Pace::FULL);
    assert_eq!(mo.row(id, at), Some(before));
    let back = mo.row(id, at + ms(m::ROW_ENTER)).expect("moving");
    assert_eq!((back.height, back.opacity), (1., 1.));
    assert!(mo.advance(ms(5_000)).is_empty());
}

#[test]
fn space_twice_quickly_completes_then_reverses_once() {
    let id = task(1);
    let mut mo = Motion::default();
    assert_eq!(mo.complete(id, ms(0), Pace::FULL), Keep::Holding);
    assert_eq!(mo.reopen(id, ms(40), Pace::FULL), Keep::InPlace);
    // Nothing is left holding, and the row settles open.
    assert_eq!(mo.holding().count(), 0);
    assert!(mo.advance(ms(5_000)).is_empty());
    assert_eq!(mo.wake(ms(5_000)), Wake::Idle);
}

#[test]
fn reopening_clears_collapses_out_of_completed_and_enters_the_open_rows() {
    let id = task(1);
    let mut mo = Motion::default();
    assert_eq!(mo.reopen(id, ms(0), Pace::FULL), Keep::Reopening);
    assert_eq!(mo.reopening().collect::<Vec<_>>(), [id]);
    let start = mo.row(id, ms(0)).expect("moving");
    assert_eq!(start, RowLook::rest(true));
    // The disc clears first; the row stays full height until then.
    let cleared = mo.row(id, ms(m::OVERLAY_OUT)).expect("moving");
    assert_eq!((cleared.fill, cleared.tick, cleared.height), (0., 0., 1.));
    let release = m::OVERLAY_OUT + m::ROW_COLLAPSE;
    assert!(mo.advance(ms(release - 1)).is_empty());
    assert_eq!(mo.advance(ms(release)), [Release::Reopened(id)]);
    let entering = mo
        .row(id, ms(release + m::ROW_ENTER / 2))
        .expect("entering");
    assert_eq!((entering.fill, entering.strike), (0., 0.));
    assert!(entering.height > 0. && entering.height < 1.);
}

#[test]
fn completing_again_while_reopening_settles_back_done() {
    let id = task(1);
    let mut mo = Motion::default();
    mo.reopen(id, ms(0), Pace::FULL);
    assert_eq!(mo.complete(id, ms(60), Pace::FULL), Keep::InPlace);
    assert_eq!(mo.reopening().count(), 0);
    assert!(mo.advance(ms(5_000)).is_empty());
    assert_eq!(mo.row(id, ms(5_000)), None);
}

#[test]
fn a_change_from_elsewhere_settles_or_stops_the_row() {
    let (a, b, c) = (task(1), task(2), task(3));
    let mut mo = Motion::default();
    mo.complete(a, ms(0), Pace::FULL);
    mo.reopen(b, ms(0), Pace::FULL);
    mo.complete(c, ms(0), Pace::FULL);
    // A was reopened on another device, B completed again; C was deleted.
    let state: BTreeMap<TaskId, bool> = [(a, false), (b, true)].into();
    mo.reconcile(ms(300), Pace::FULL, |id| state.get(&id).copied());
    assert_eq!(mo.holding().count(), 0);
    assert_eq!(mo.reopening().count(), 0);
    assert_eq!(mo.row(c, ms(300)), None);
    assert!(mo.advance(ms(5_000)).is_empty());
    assert_eq!(mo.wake(ms(5_000)), Wake::Idle);
}

#[test]
fn switching_rooms_puts_everything_at_rest() {
    let mut mo = Motion::default();
    mo.complete(task(1), ms(0), Pace::FULL);
    mo.reopen(task(2), ms(0), Pace::FULL);
    mo.settle_all();
    assert_eq!(mo.holding().count() + mo.reopening().count(), 0);
    assert_eq!(mo.wake(ms(0)), Wake::Idle);
}

#[test]
fn the_sun_glides_along_the_sun_curve_and_retargets_from_where_it_is() {
    let mut mo = Motion::default();
    mo.aim_sun(MY_DAY, Some(0.25), ms(0), Pace::FULL);
    // The first aim places it.
    assert_eq!(mo.sun(ms(0)), Some(0.25));
    assert_eq!(mo.wake(ms(0)), Wake::Idle);
    mo.aim_sun(MY_DAY, Some(0.5), ms(1_000), Pace::FULL);
    assert_eq!(mo.sun(ms(1_000)), Some(0.25));
    assert_eq!(mo.wake(ms(1_000)), Wake::Frame);
    // The symmetric SUN curve is halfway at half time, and slow at both ends.
    let half = mo.sun(ms(1_000 + m::SUN_TRAVEL / 2)).expect("sun");
    assert!(close(half, 0.375), "{half}");
    let early = mo.sun(ms(1_000 + m::SUN_TRAVEL / 10)).expect("sun");
    assert!(early - 0.25 < 0.025 * 0.5, "{early}");
    // A quick second change retargets from the current place, without a jump.
    let at = ms(1_000 + m::SUN_TRAVEL / 2);
    mo.aim_sun(MY_DAY, Some(0.75), at, Pace::FULL);
    assert!(close(mo.sun(at).expect("sun"), half));
    assert_eq!(mo.sun(at + ms(m::SUN_TRAVEL)), Some(0.75));
    assert_eq!(mo.wake(at + ms(m::SUN_TRAVEL)), Wake::Idle);
    // Aiming at the same place again changes nothing.
    mo.aim_sun(MY_DAY, Some(0.75), ms(9_000), Pace::FULL);
    assert_eq!(mo.wake(ms(9_000)), Wake::Idle);
}

#[test]
fn the_sun_snaps_in_a_new_room_with_reduced_motion_and_when_it_appears() {
    let mut mo = Motion::default();
    mo.aim_sun(MY_DAY, Some(0.25), ms(0), Pace::FULL);
    mo.aim_sun(
        RoomKey::Smart(crate::model::Smart::All),
        Some(0.8),
        ms(10),
        Pace::FULL,
    );
    assert_eq!(mo.sun(ms(10)), Some(0.8));
    mo.aim_sun(MY_DAY, Some(0.25), ms(20), Pace::FULL);
    mo.aim_sun(MY_DAY, Some(0.5), ms(30), Pace::REDUCED);
    assert_eq!(mo.sun(ms(30)), Some(0.5));
    assert_eq!(mo.wake(ms(30)), Wake::Idle);
    mo.aim_sun(MY_DAY, None, ms(40), Pace::FULL);
    assert_eq!(mo.sun(ms(40)), None);
    mo.aim_sun(MY_DAY, Some(0.), ms(50), Pace::FULL);
    assert_eq!(mo.sun(ms(50)), Some(0.));
}

#[test]
fn a_placed_sun_glides_back_from_where_it_was_put() {
    let mut mo = Motion::default();
    mo.aim_sun(MY_DAY, Some(11. / 12.), ms(0), Pace::FULL);
    // The day finishes: the dusk scene takes the sun, with no travel to draw.
    mo.place_sun(MY_DAY, 1., ms(100));
    assert_eq!(mo.sun(ms(100)), Some(1.));
    assert_eq!(mo.wake(ms(100)), Wake::Idle);
    // A task reopens: it glides back from the end of the arc.
    mo.aim_sun(MY_DAY, Some(11. / 12.), ms(200), Pace::FULL);
    assert_eq!(mo.sun(ms(200)), Some(1.));
    assert_eq!(mo.wake(ms(200)), Wake::Frame);
}

#[test]
fn dusk_falls_with_its_cues_and_lifts_briskly() {
    let mut mo = Motion::default();
    mo.aim_dusk(MY_DAY, false, ms(0), Pace::FULL);
    assert_eq!(mo.dusk(ms(0)), DuskLook::rest(false));
    assert_eq!(mo.wake(ms(0)), Wake::Idle);
    // The last task has left: dusk falls over DUSK.
    mo.aim_dusk(MY_DAY, true, ms(0), Pace::FULL);
    assert_eq!(mo.wake(ms(0)), Wake::Frame);
    let at = |share: f32| mo.dusk(ms((m::DUSK as f32 * share) as u32));
    // The ink switches and the night begins at its cue; the hand-over enters after its own, staggered.
    assert!(!at(m::DUSK_NIGHT_AT - 0.05).night_ink());
    assert!(at(m::DUSK_NIGHT_AT + 0.01).night_ink());
    assert_eq!(at(m::DUSK_NIGHT_AT - 0.05).night(), 0.);
    assert_eq!(
        at(m::DUSK_HANDOVER_AT - 0.01).handover(0),
        (0., m::DUSK_HANDOVER_RISE)
    );
    let (first, rise) = at(m::DUSK_HANDOVER_AT + 0.1).handover(0);
    let (second, _) = at(m::DUSK_HANDOVER_AT + 0.1).handover(1);
    assert!(first > second && second > 0. && rise > 0. && rise < m::DUSK_HANDOVER_RISE);
    // The SUN curve: half way at half time.
    let half = at(0.5);
    assert!(close(half.scene(), 0.5) && close(half.height(), 0.5));
    let done = mo.dusk(ms(m::DUSK));
    assert!(done.settled() && done.scene() == 1. && done.night() == 1.);
    assert_eq!(done.handover(m::STAGGER_MAX_ITEMS + 3), (1., 0.));
    assert_eq!(mo.wake(ms(m::DUSK)), Wake::Idle);
    // A task reopens: it turns back from there, over the brisker ROOM_HUE.
    let back = ms(m::DUSK + 1_000);
    mo.aim_dusk(MY_DAY, false, back, Pace::FULL);
    assert_eq!(mo.dusk(back).scene(), 1.);
    assert!(mo.dusk(back + ms(m::ROOM_HUE)).settled());
    assert_eq!(mo.dusk(back + ms(m::ROOM_HUE)).scene(), 0.);
}

#[test]
fn turning_back_part_way_retargets_from_where_the_dusk_is() {
    let mut mo = Motion::default();
    mo.aim_dusk(MY_DAY, false, ms(0), Pace::FULL);
    mo.aim_dusk(MY_DAY, true, ms(0), Pace::FULL);
    let at = ms(m::DUSK / 3);
    let before = mo.dusk(at).scene();
    mo.aim_dusk(MY_DAY, false, at, Pace::FULL);
    assert!(close(mo.dusk(at).scene(), before));
    assert!(mo.dusk(at + ms(m::ROOM_HUE)).settled());
}

#[test]
fn reduced_dusk_crossfades_without_growing_or_rising() {
    let mut mo = Motion::default();
    mo.aim_dusk(MY_DAY, false, ms(0), Pace::REDUCED);
    mo.aim_dusk(MY_DAY, true, ms(0), Pace::REDUCED);
    let mid = mo.dusk(ms(m::REDUCED_FADE / 2));
    assert_eq!(mid.height(), 1.);
    assert!(mid.scene() > 0. && mid.scene() < 1.);
    assert_eq!(mid.handover(3).1, 0.);
    assert!(mo.dusk(ms(m::REDUCED_FADE)).settled());
    // Another room starts at rest.
    mo.aim_dusk(
        RoomKey::Smart(crate::model::Smart::All),
        false,
        ms(10),
        Pace::FULL,
    );
    assert_eq!(mo.dusk(ms(10)), DuskLook::rest(false));
}

#[test]
fn a_row_the_list_no_longer_shows_stops_at_once() {
    let (held, reopened, entering) = (task(1), task(2), task(3));
    let mut mo = Motion::default();
    mo.complete(entering, ms(0), Pace::FULL);
    mo.complete(held, ms(100), Pace::FULL);
    mo.reopen(reopened, ms(RELEASE_AT - 100), Pace::FULL);
    assert_eq!(mo.advance(ms(RELEASE_AT)), [Release::Completed(entering)]);
    let now = ms(RELEASE_AT);
    // Nothing the list doesn't show: the deleted (tombstoned) held task stops and is let go, a task
    // reopened where it isn't shown goes straight to entering the open rows, and the released row
    // with no place in a closed Completed section stops entering.
    assert!(mo.retain_shown(now, |_| false));
    assert_eq!(mo.row(held, now), None);
    assert_eq!(mo.row(entering, now), None);
    assert_eq!(mo.holding().count() + mo.reopening().count(), 0);
    let enters = mo.row(reopened, now).expect("entering");
    assert_eq!((enters.height, enters.fill), (0., 0.));
    // Not shown there either: it stops, and that lets nothing go.
    assert!(!mo.retain_shown(now, |_| false));
    assert_eq!(mo.wake(now), Wake::Idle);
}

#[test]
fn rows_rising_from_past_the_rendered_slots_are_drawn_with_them() {
    // Ten rows, slots 0..6 rendered; row 2 is half collapsed, so rows 3.. are drawn half a row up.
    let mut heights = [1.; 10];
    heights[2] = 0.5;
    let up = lifts(heights);
    assert_eq!(rows_rising_into(&up, 6).collect::<Vec<_>>(), [6]);
    // Rows 2 and 4 fully collapsed: rows from 5 on are two rows up.
    heights[2] = 0.;
    heights[4] = 0.;
    let up = lifts(heights);
    assert_eq!(rows_rising_into(&up, 6).collect::<Vec<_>>(), [6, 7]);
    // Nothing collapsing: nothing past the rendered slots shows, and nothing past the end exists.
    assert_eq!(rows_rising_into(&lifts([1.; 10]), 6).count(), 0);
    assert_eq!(rows_rising_into(&up, 10).count(), 0);
}

#[test]
fn a_list_scrolled_to_its_end_comes_back_with_a_collapsing_row() {
    let row = 50.;
    // Ten rows of 50 in a 200 tall viewport: scrolled to the end, 300 down.
    let mut heights = [1.; 10];
    assert_eq!(anchored_scroll(300., 200., row, &heights), None);
    // A row half collapsed: the content is 25 shorter, so the scroll comes back 25.
    heights[3] = 0.5;
    assert_eq!(anchored_scroll(300., 200., row, &heights), Some(275.));
    // Fully collapsed: 50 back, exactly where the list ends once the row is released.
    heights[3] = 0.;
    assert_eq!(anchored_scroll(300., 200., row, &heights), Some(250.));
    assert_eq!(anchored_scroll(250., 200., row, &[1.; 9]), None);
    // Scrolled less far, or at the top, nothing moves; content shorter than the viewport ends at 0.
    assert_eq!(anchored_scroll(100., 200., row, &heights), None);
    assert_eq!(anchored_scroll(0., 200., row, &heights), None);
    assert_eq!(anchored_scroll(20., 600., row, &heights), Some(0.));
}

#[test]
fn j_and_k_reach_only_the_hand_over_tasks_that_have_begun_to_enter() {
    let shell = crate::fixtures::finished().shell();
    let next = shell.room.handover().expect("hands over");
    let all: Vec<TaskId> = next.tasks().map(|t| t.id).collect();
    let mut mo = Motion::default();
    mo.aim_dusk(MY_DAY, false, ms(0), Pace::FULL);
    mo.aim_dusk(MY_DAY, true, ms(0), Pace::FULL);
    let nav = |t: u32| {
        let dusk = mo.dusk(ms(t));
        shell.nav_tasks_shown(|c| dusk.handover(c).0 > 0.)
    };
    // Dusk has fallen half way: the hand-over isn't there yet, so there's nothing to select.
    assert!(nav(m::DUSK / 2).is_empty());
    let cue = (m::DUSK as f32 * m::DUSK_HANDOVER_AT) as u32;
    assert!(nav(cue).is_empty());
    // Just after the first task has begun to enter (one stagger after the label), only it.
    assert_eq!(nav(cue + m::STAGGER + 5), [all[0]]);
    // Entered: all of them, in order.
    assert_eq!(nav(m::DUSK), all);
}

#[test]
fn rows_below_a_collapsing_row_rise_by_the_share_that_has_gone() {
    assert_eq!(lifts([1., 0.25, 1., 0.5, 1.]), [0., 0., 0.75, 0.75, 1.25]);
    assert!(lifts([]).is_empty());
}

/// Something the owner or the sync engine does, at some time after the previous one.
#[derive(Clone, Debug)]
enum Op {
    /// The owner toggles task `n` (click or Space).
    Toggle(u8),
    /// Another device flips task `n`.
    Elsewhere(u8),
    /// Task `n` is deleted. The replica keeps its tombstone, which still says whether it was done; the
    /// list no longer shows it.
    Delete(u8),
    /// The owner switches rooms.
    SwitchRoom,
}

fn op() -> impl Strategy<Value = (u32, Op)> {
    let n = 0u8..3;
    (
        prop_oneof![0u32..60, 0u32..1_200],
        prop_oneof![
            6 => n.clone().prop_map(Op::Toggle),
            2 => n.clone().prop_map(Op::Elsewhere),
            1 => n.prop_map(Op::Delete),
            1 => Just(Op::SwitchRoom),
        ],
    )
}

fn in_range(look: RowLook) -> bool {
    let unit = |v: f32| (0. ..=1.).contains(&v);
    unit(look.fill)
        && unit(look.tick)
        && unit(look.strike)
        && unit(look.dim)
        && unit(look.opacity)
        && unit(look.height)
        && (m::CHECK_FILL_FROM_SCALE..1.2).contains(&look.scale)
}

/// What the eye sees of a look: the scale and the tick don't show while the disc is empty.
fn seen(look: RowLook) -> RowLook {
    if look.fill == 0. {
        RowLook {
            scale: 1.,
            tick: 0.,
            ..look
        }
    } else {
        look
    }
}

proptest! {
    /// Any sequence of toggles, changes from elsewhere, deletions and room switches, at any speed: every
    /// row stays in range and toggles never jump; a task is held or reopening only while its state says it
    /// was just completed or reopened; releases always agree with the state; and once things go quiet,
    /// everything converges to rest with no frames or timers left.
    #[test]
    fn the_choreography_converges_to_rest_and_stays_consistent(
        ops in proptest::collection::vec(op(), 1..40),
        reduced in any::<bool>(),
    ) {
        let pace = Pace { reduced };
        let mut mo = Motion::default();
        let mut now = Duration::ZERO;
        // Each task's state, and where the list shows it (done = in Completed).
        let mut done: BTreeMap<TaskId, bool> = (0..3).map(|n| (task(n), false)).collect();
        let mut placed = done.clone();
        let mut deleted = BTreeSet::new();
        let check = |mo: &Motion, now: Duration, done: &BTreeMap<TaskId, bool>, placed: &BTreeMap<TaskId, bool>, deleted: &BTreeSet<TaskId>| -> Result<(), TestCaseError> {
            let holding: BTreeSet<TaskId> = mo.holding().collect();
            let reopening: BTreeSet<TaskId> = mo.reopening().collect();
            for (&id, &d) in done.iter() {
                if let Some(look) = mo.row(id, now) {
                    prop_assert!(in_range(look), "{look:?}");
                }
                if deleted.contains(&id) {
                    // Gone from the list: nothing moves for it, nothing keeps it.
                    prop_assert_eq!(mo.row(id, now), None);
                    prop_assert!(!holding.contains(&id) && !reopening.contains(&id));
                } else if holding.contains(&id) {
                    prop_assert!(d && !placed[&id]);
                } else if reopening.contains(&id) {
                    prop_assert!(!d && placed[&id]);
                } else {
                    prop_assert_eq!(placed[&id], d);
                }
            }
            Ok(())
        };
        for (gap, op) in ops {
            now += ms(gap);
            for r in mo.advance(now) {
                match r {
                    Release::Completed(id) => {
                        prop_assert!(done.get(&id) == Some(&true) && !placed[&id]);
                        placed.insert(id, true);
                    }
                    Release::Reopened(id) => {
                        prop_assert!(done.get(&id) == Some(&false) && placed[&id]);
                        placed.insert(id, false);
                    }
                }
            }
            check(&mo, now, &done, &placed, &deleted)?;
            match op {
                Op::Toggle(n) => {
                    let id = task(n);
                    if deleted.contains(&id) {
                        continue;
                    }
                    let d = done[&id];
                    let before = mo.row(id, now).unwrap_or_else(|| RowLook::rest(placed[&id]));
                    done.insert(id, !d);
                    let keep = if d { mo.reopen(id, now, pace) } else { mo.complete(id, now, pace) };
                    match keep {
                        Keep::Holding => prop_assert!(!placed[&id]),
                        Keep::Reopening => prop_assert!(placed[&id]),
                        Keep::InPlace => prop_assert_eq!(placed[&id], !d),
                    }
                    let after = mo.row(id, now).expect("a toggle starts motion");
                    if !reduced {
                        prop_assert_eq!(seen(after), seen(before), "a toggle jumped");
                    }
                }
                Op::Elsewhere(n) => {
                    let id = task(n);
                    if !deleted.contains(&id) {
                        done.insert(id, !done[&id]);
                    }
                }
                Op::Delete(n) => {
                    deleted.insert(task(n));
                }
                Op::SwitchRoom => mo.settle_all(),
            }
            // As the view does: the tombstone still answers, and the derived list doesn't show it.
            mo.reconcile(now, pace, |id| done.get(&id).copied());
            // When that lets a task go the view derives again, and the list still doesn't show it.
            while mo.retain_shown(now, |id| !deleted.contains(&id)) {}
            // The list follows the state wherever no choreography holds a task back.
            let holding: BTreeSet<TaskId> = mo.holding().collect();
            let reopening: BTreeSet<TaskId> = mo.reopening().collect();
            for (id, d) in &done {
                if !holding.contains(id) && !reopening.contains(id) {
                    placed.insert(*id, *d);
                }
            }
            check(&mo, now, &done, &placed, &deleted)?;
        }
        // Quiet: everything is released and at rest, with no frames and no timers.
        now += ms(10_000);
        for r in mo.advance(now) {
            match r {
                Release::Completed(id) => { placed.insert(id, true); }
                Release::Reopened(id) => { placed.insert(id, false); }
            }
        }
        prop_assert_eq!(mo.wake(now), Wake::Idle);
        prop_assert_eq!(mo.holding().count() + mo.reopening().count(), 0);
        for (id, d) in &done {
            if !deleted.contains(id) {
                prop_assert_eq!(placed[id], *d);
            }
        }
        for id in done.keys() {
            prop_assert_eq!(mo.row(*id, now), None);
        }
    }

    /// However often the count changes, the sun never leaves the arc between its old and new places, and
    /// it comes to rest exactly where the last change put it.
    #[test]
    fn the_sun_converges_to_its_last_target(
        aims in proptest::collection::vec((0u32..900, 0u32..=12), 1..20),
    ) {
        let mut mo = Motion::default();
        let mut now = Duration::ZERO;
        let mut last = 0.;
        for (gap, done) in aims {
            now += ms(gap);
            last = done as f32 / 12.;
            mo.aim_sun(MY_DAY, Some(last), now, Pace::FULL);
            let s = mo.sun(now).expect("sun");
            prop_assert!((0. ..=1.).contains(&s));
        }
        now += ms(m::SUN_TRAVEL);
        prop_assert_eq!(mo.sun(now), Some(last));
        prop_assert_eq!(mo.wake(now), Wake::Idle);
    }
}
