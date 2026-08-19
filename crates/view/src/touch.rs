//! Fingers, turned into camera motion.
//!
//! The desktop viewer spends eleven keys and two mouse buttons on navigation.
//! A phone has neither, so the whole of it has to come out of what a hand can
//! do on glass: one finger, two fingers, and time.
//!
//! The split is by finger count, which is the only division a user never has to
//! be taught. One finger turns the head. Two fingers move the body — the pair's
//! centre is a joystick and the gap between them is a throttle, so the same
//! grip that flies you somewhere also decides how fast. A double tap puts you
//! back where the whole plant is visible, which is the only way out of having
//! flown inside a wall.
//!
//! # Everything here is in CSS pixels
//!
//! Winit reports touches in physical pixels, and a phone reports a device pixel
//! ratio of 3 against a laptop's 1. A threshold in physical pixels would
//! therefore be three different gestures on three different screens: the same
//! movement of the hand would turn three times as far, and a tap would be
//! allowed a third as much wander. A CSS pixel is roughly the same physical size
//! everywhere, which is the unit the hand actually knows about, so callers
//! divide by the pixel ratio on the way in.

use crate::Instant;

/// How far a finger may wander and still count as a tap.
///
/// Ten CSS pixels is under a millimetre and a half on a phone. A finger that is
/// really starting a drag beats that within the first frame or two.
const TAP_SLOP: f32 = 10.0;

/// How long a finger may stay down and still count as a tap, in seconds.
const TAP_TIME: f32 = 0.25;

/// How long after a tap lifts a second one still reads as a double tap.
const DOUBLE_TAP_TIME: f32 = 0.30;

/// Radians of rotation per CSS pixel of one-finger drag.
///
/// A finger travels far less than a mouse does, because the hand cannot leave
/// the screen and pick up again, so this is roughly double the desktop's 0.0022
/// per pixel. At this rate a swipe across half of a 393-point iPhone turns about
/// 56 degrees, making a full turn three comfortable swipes. This is the number
/// to change if it feels wrong; nothing else depends on it.
const LOOK: f32 = 0.005;

/// Centroid travel, in CSS pixels, before the two-finger joystick registers.
///
/// Two fingers never land at the same instant and never lift at the same
/// instant. Without a deadzone the centroid jumps at both edges of every pinch,
/// and the camera lurches each time the user only meant to change speed.
const FLY_DEADZONE: f32 = 12.0;

/// Centroid travel, in CSS pixels, at which the joystick is fully deflected.
///
/// Ninety is about a quarter of the width of a phone held in one hand, which
/// keeps full speed reachable with a thumb rather than the whole arm.
const FLY_RANGE: f32 = 90.0;

/// Speed multiplier at full deflection.
///
/// The desktop's shift key gives 5x, which is far too much on a control that
/// cannot be released precisely. Fine movement matters more here, and the pinch
/// is there for anyone who genuinely wants to cross the map.
const FLY_BOOST: f32 = 2.0;

/// Fractional change in finger separation ignored as noise.
///
/// Fingers tremble. Without this the base speed drifts while the user is
/// holding still, which reads as the viewer having a mind of its own.
const PINCH_DEADZONE: f32 = 0.02;

/// Something the camera should do now.
///
/// Flight is deliberately not in here. It is a sustained state rather than an
/// event, so it is read once per frame from [`Touches::fly`] instead of being
/// delivered on whichever touch happened to move last.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    /// Turn by this much drag, in CSS pixels.
    Look { dx: f32, dy: f32 },
    /// Multiply the base speed by this ratio.
    Speed(f32),
    /// Put the camera back where the whole plant is visible.
    Reframe,
}

/// Which phase of its life a touch is in.
///
/// A local copy of winit's, so this module can be reasoned about and tested
/// without a window, an event loop or a browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// The finger landed.
    Started,
    /// The finger moved.
    Moved,
    /// The finger lifted, or the browser took the gesture away.
    Ended,
}

/// One finger, from the moment it lands to the moment it lifts.
struct Finger {
    /// Winit's pointer id, stable for this finger's whole life. That stability
    /// is what makes multi-touch tractable at all.
    id: u64,
    /// Where it is now, CSS pixels.
    at: (f32, f32),
    /// When it landed.
    down: Instant,
    /// Total distance travelled, CSS pixels.
    ///
    /// Accumulated rather than measured start to end, so a finger that wanders
    /// away and comes back is not mistaken for one that never moved.
    travel: f32,
    /// Whether another finger was ever down at the same time.
    ///
    /// Without this, lifting from a two-finger gesture produces two taps, and
    /// two taps in quick succession are a double tap — so every pinch would end
    /// by throwing the camera back to the overview.
    shared: bool,
}

/// The live touch state.
#[derive(Default)]
pub struct Touches {
    /// Active fingers, in the order they landed.
    fingers: Vec<Finger>,
    /// Centroid when the current two-finger gesture began, CSS pixels.
    origin: Option<(f32, f32)>,
    /// Finger separation at the last pinch sample, CSS pixels.
    span: Option<f32>,
    /// When the last qualifying tap lifted.
    tapped: Option<Instant>,
}

impl Touches {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget every finger.
    ///
    /// Switching tab mid-drag means the lift is delivered to another page and
    /// never arrives here, and the camera would still be flying on return.
    pub fn clear(&mut self) {
        self.fingers.clear();
        self.origin = None;
        self.span = None;
    }

    /// Feed one touch event. `at` is in CSS pixels.
    pub fn on_touch(
        &mut self,
        id: u64,
        phase: Phase,
        at: (f32, f32),
        now: Instant,
    ) -> Option<Action> {
        match phase {
            Phase::Started => {
                let shared = !self.fingers.is_empty();
                for finger in &mut self.fingers {
                    finger.shared = true;
                }
                self.fingers.push(Finger {
                    id,
                    at,
                    down: now,
                    travel: 0.0,
                    shared,
                });
                self.begin_pair();
                None
            }
            Phase::Moved => self.moved(id, at),
            Phase::Ended => self.ended(id, now),
        }
    }

    /// Sustained flight, as (forward, right, throttle).
    ///
    /// Forward and right are a unit direction and the throttle is a speed
    /// multiplier, because [`crate::camera::Camera::fly`] normalises its axes
    /// and takes magnitude separately.
    #[must_use]
    pub fn fly(&self) -> Option<(f32, f32, f32)> {
        let origin = self.origin?;
        if self.fingers.len() < 2 {
            return None;
        }
        let centre = self.centroid()?;
        let (dx, dy) = (centre.0 - origin.0, centre.1 - origin.1);
        let distance = dx.hypot(dy);
        if distance <= FLY_DEADZONE {
            return None;
        }
        let throttle =
            ((distance - FLY_DEADZONE) / (FLY_RANGE - FLY_DEADZONE)).clamp(0.0, 1.0) * FLY_BOOST;
        // Screen y grows downward, so pushing the pair up the glass — a
        // negative dy — has to mean forward.
        Some((-dy / distance, dx / distance, throttle))
    }

    /// Whether any finger is down.
    #[must_use]
    pub fn active(&self) -> bool {
        !self.fingers.is_empty()
    }

    fn moved(&mut self, id: u64, at: (f32, f32)) -> Option<Action> {
        let index = self.fingers.iter().position(|f| f.id == id)?;
        let previous = self.fingers[index].at;
        {
            let finger = &mut self.fingers[index];
            finger.travel += (at.0 - previous.0).hypot(at.1 - previous.1);
            finger.at = at;
        }

        match self.fingers.len() {
            // One finger turns the head, and only if it has been alone for its
            // whole life. A finger left over from a pinch keeps flying the
            // camera it was already flying rather than suddenly steering it.
            1 if !self.fingers[0].shared => Some(Action::Look {
                dx: at.0 - previous.0,
                dy: at.1 - previous.1,
            }),
            // Two fingers do two things at once. The centroid flies, which
            // `fly` reads each frame, and the gap between them sets the base
            // speed, which is an event because it is a change rather than a
            // state.
            2.. => self.pinch(),
            _ => None,
        }
    }

    fn ended(&mut self, id: u64, now: Instant) -> Option<Action> {
        let index = self.fingers.iter().position(|f| f.id == id)?;
        let finger = self.fingers.remove(index);

        // Dropping below two fingers ends the pair. Putting a finger back down
        // later starts a fresh one from wherever the hand now is, rather than
        // measuring against a centroid from a gesture that is over.
        if self.fingers.len() < 2 {
            self.origin = None;
            self.span = None;
        } else {
            self.begin_pair();
        }

        let tap = !finger.shared
            && finger.travel <= TAP_SLOP
            && (now - finger.down).as_secs_f32() <= TAP_TIME;
        if !tap {
            return None;
        }

        match self.tapped {
            Some(previous) if (now - previous).as_secs_f32() <= DOUBLE_TAP_TIME => {
                // Consumed, so a third tap begins a new pair rather than firing
                // again on every tap after the second.
                self.tapped = None;
                Some(Action::Reframe)
            }
            _ => {
                self.tapped = Some(now);
                None
            }
        }
    }

    /// Anchor the two-finger gesture on the fingers that are down now.
    fn begin_pair(&mut self) {
        if self.fingers.len() < 2 {
            return;
        }
        self.origin = self.centroid();
        self.span = self.separation();
    }

    fn pinch(&mut self) -> Option<Action> {
        let span = self.separation()?;
        let previous = self.span?;
        if previous <= f32::EPSILON {
            return None;
        }
        let ratio = span / previous;
        if (ratio - 1.0).abs() <= PINCH_DEADZONE {
            return None;
        }
        self.span = Some(span);
        Some(Action::Speed(ratio))
    }

    /// Mean position of the first two fingers, CSS pixels.
    fn centroid(&self) -> Option<(f32, f32)> {
        let a = self.fingers.first()?.at;
        let b = self.fingers.get(1)?.at;
        Some(((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5))
    }

    /// Distance between the first two fingers, CSS pixels.
    fn separation(&self) -> Option<f32> {
        let a = self.fingers.first()?.at;
        let b = self.fingers.get(1)?.at;
        Some((a.0 - b.0).hypot(a.1 - b.1))
    }
}

/// Radians to turn for a drag of this many CSS pixels.
#[must_use]
pub fn look_radians(pixels: f32) -> f32 {
    pixels * LOOK
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(now: Instant, seconds: f32) -> Instant {
        now + std::time::Duration::from_secs_f32(seconds)
    }

    #[test]
    fn one_finger_drag_turns_the_camera() {
        let mut touches = Touches::new();
        let t = Instant::now();
        assert_eq!(touches.on_touch(1, Phase::Started, (100.0, 100.0), t), None);
        assert_eq!(
            touches.on_touch(1, Phase::Moved, (140.0, 110.0), at(t, 0.02)),
            Some(Action::Look { dx: 40.0, dy: 10.0 })
        );
    }

    #[test]
    fn two_fingers_fly_and_the_pair_pushed_up_goes_forward() {
        let mut touches = Touches::new();
        let t = Instant::now();
        touches.on_touch(1, Phase::Started, (100.0, 300.0), t);
        touches.on_touch(2, Phase::Started, (200.0, 300.0), t);
        assert_eq!(touches.fly(), None, "resting fingers must not fly");

        // Slide both fingers a long way up the glass.
        touches.on_touch(1, Phase::Moved, (100.0, 200.0), at(t, 0.05));
        touches.on_touch(2, Phase::Moved, (200.0, 200.0), at(t, 0.05));
        let (forward, right, throttle) = touches.fly().expect("the pair has left the deadzone");
        assert!(forward > 0.99, "up the screen is forward, got {forward}");
        assert!(right.abs() < 0.01, "no sideways component, got {right}");
        assert!(throttle > 0.0);
    }

    #[test]
    fn spreading_the_fingers_raises_the_speed() {
        let mut touches = Touches::new();
        let t = Instant::now();
        touches.on_touch(1, Phase::Started, (100.0, 300.0), t);
        touches.on_touch(2, Phase::Started, (200.0, 300.0), t);
        // 100 px apart becomes 200 px apart.
        let action = touches.on_touch(2, Phase::Moved, (300.0, 300.0), at(t, 0.05));
        match action {
            Some(Action::Speed(ratio)) => assert!((ratio - 2.0).abs() < 0.001, "got {ratio}"),
            other => panic!("expected a speed change, got {other:?}"),
        }
    }

    #[test]
    fn a_double_tap_reframes() {
        let mut touches = Touches::new();
        let t = Instant::now();
        touches.on_touch(1, Phase::Started, (50.0, 50.0), t);
        assert_eq!(touches.on_touch(1, Phase::Ended, (50.0, 50.0), at(t, 0.05)), None);
        touches.on_touch(2, Phase::Started, (52.0, 51.0), at(t, 0.15));
        assert_eq!(
            touches.on_touch(2, Phase::Ended, (52.0, 51.0), at(t, 0.20)),
            Some(Action::Reframe)
        );
    }

    /// The bug this exists to prevent: two fingers lifting off a pinch look
    /// exactly like two quick taps, and would reframe the camera every time.
    #[test]
    fn lifting_from_a_pinch_is_not_a_double_tap() {
        let mut touches = Touches::new();
        let t = Instant::now();
        touches.on_touch(1, Phase::Started, (100.0, 300.0), t);
        touches.on_touch(2, Phase::Started, (200.0, 300.0), t);
        assert_eq!(touches.on_touch(1, Phase::Ended, (100.0, 300.0), at(t, 0.05)), None);
        assert_eq!(touches.on_touch(2, Phase::Ended, (200.0, 300.0), at(t, 0.08)), None);
    }

    #[test]
    fn a_slow_press_is_not_a_tap() {
        let mut touches = Touches::new();
        let t = Instant::now();
        touches.on_touch(1, Phase::Started, (50.0, 50.0), t);
        assert_eq!(touches.on_touch(1, Phase::Ended, (50.0, 50.0), at(t, 0.5)), None);
        touches.on_touch(2, Phase::Started, (50.0, 50.0), at(t, 0.6));
        assert_eq!(
            touches.on_touch(2, Phase::Ended, (50.0, 50.0), at(t, 0.65)),
            None,
            "the first press never counted, so this is a first tap"
        );
    }

    #[test]
    fn a_drag_is_not_a_tap() {
        let mut touches = Touches::new();
        let t = Instant::now();
        touches.on_touch(1, Phase::Started, (50.0, 50.0), t);
        touches.on_touch(1, Phase::Moved, (90.0, 50.0), at(t, 0.05));
        assert_eq!(touches.on_touch(1, Phase::Ended, (90.0, 50.0), at(t, 0.10)), None);
    }
}
