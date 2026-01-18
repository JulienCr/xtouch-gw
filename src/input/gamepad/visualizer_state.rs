//! State structures for gamepad visualizer
//!
//! Tracks raw and normalized values for all connected controllers (XInput and gilrs)

use std::collections::HashMap;
use std::time::Instant;

use gilrs::GamepadId;

/// Maximum number of points in a trail to prevent memory issues
const MAX_TRAIL_POINTS: usize = 1000;

/// Minimum distance between trail points to avoid clustering (in normalized units)
const MIN_TRAIL_DISTANCE: f32 = 0.005;

/// Trail data for a stick visualization
#[derive(Debug, Default, Clone)]
pub struct StickTrail {
    /// Trail points for raw values plot (gilrs only)
    pub raw_points: Vec<egui::Pos2>,
    /// Trail points for normalized values plot
    pub normalized_points: Vec<egui::Pos2>,
}

impl StickTrail {
    /// Add a point to the raw trail (for gilrs dual-plot mode)
    pub fn add_raw_point(&mut self, x: f32, y: f32) {
        let new_point = egui::pos2(x, y);
        if self.should_add_point(&self.raw_points, new_point) {
            self.raw_points.push(new_point);
            if self.raw_points.len() > MAX_TRAIL_POINTS {
                self.raw_points.remove(0);
            }
        }
    }

    /// Add a point to the normalized trail
    pub fn add_normalized_point(&mut self, x: f32, y: f32) {
        let new_point = egui::pos2(x, y);
        if self.should_add_point(&self.normalized_points, new_point) {
            self.normalized_points.push(new_point);
            if self.normalized_points.len() > MAX_TRAIL_POINTS {
                self.normalized_points.remove(0);
            }
        }
    }

    /// Check if a new point should be added (minimum distance check)
    fn should_add_point(&self, points: &[egui::Pos2], new_point: egui::Pos2) -> bool {
        if let Some(last) = points.last() {
            let dx = new_point.x - last.x;
            let dy = new_point.y - last.y;
            let dist = (dx * dx + dy * dy).sqrt();
            dist >= MIN_TRAIL_DISTANCE
        } else {
            true // Always add the first point
        }
    }

    /// Clear all trail points
    pub fn clear(&mut self) {
        self.raw_points.clear();
        self.normalized_points.clear();
    }
}

/// Identifies the source/backend of a controller
#[derive(Debug, Clone)]
pub enum ControllerBackend {
    /// XInput controller (Xbox, etc.)
    XInput { user_index: u32, packet_number: u32 },
    /// HID controller via gilrs
    Gilrs { gamepad_id: GamepadId, name: String },
}

/// State for all controllers being visualized
#[derive(Debug)]
pub struct VisualizerState {
    /// XInput controllers (fixed slots 0-3)
    pub xinput_controllers: Vec<ControllerState>,
    /// Gilrs/HID controllers (dynamic, keyed by GamepadId)
    pub gilrs_controllers: HashMap<GamepadId, ControllerState>,
}

impl VisualizerState {
    /// Create new state with 4 XInput controller slots and empty gilrs map
    pub fn new() -> Self {
        Self {
            xinput_controllers: (0..4)
                .map(|i| {
                    ControllerState::new(ControllerBackend::XInput {
                        user_index: i,
                        packet_number: 0,
                    })
                })
                .collect(),
            gilrs_controllers: HashMap::new(),
        }
    }

    /// Update state from XInput gamepad state
    pub fn update_from_xinput(&mut self, user_index: u32, state: &rusty_xinput::XInputState) {
        let Some(controller) = self.xinput_controllers.get_mut(user_index as usize) else {
            return;
        };
        controller.connected = true;
        controller.backend = ControllerBackend::XInput {
            user_index,
            packet_number: state.raw.dwPacketNumber,
        };
        controller.last_update = Instant::now();

        let gp = &state.raw.Gamepad;
        controller.left_stick.raw_x = Some(gp.sThumbLX);
        controller.left_stick.raw_y = Some(gp.sThumbLY);
        controller.right_stick.raw_x = Some(gp.sThumbRX);
        controller.right_stick.raw_y = Some(gp.sThumbRY);
        controller.left_trigger.raw = Some(state.left_trigger());
        controller.right_trigger.raw = Some(state.right_trigger());
        controller.buttons.update_from_xinput(gp.wButtons);
    }

    /// Update state from gilrs gamepad
    ///
    /// # Arguments
    /// * `gamepad` - The gilrs gamepad to read state from
    /// * `capture` - Capture button state (tracked separately, not in gilrs standard mapping)
    pub fn update_from_gilrs(&mut self, gamepad: &gilrs::Gamepad, capture: bool) {
        use gilrs::Axis;
        let id = gamepad.id();
        let controller = self.gilrs_controllers.entry(id).or_insert_with(|| {
            ControllerState::new(ControllerBackend::Gilrs {
                gamepad_id: id,
                name: gamepad.name().to_string(),
            })
        });
        controller.connected = true;
        controller.last_update = Instant::now();

        // Gilrs provides per-axis values (-1.0 to 1.0) forming a square
        // Map square to circle (same radial behavior as XInput)
        use super::visualizer::normalize::normalize_gilrs_stick;

        // Store raw gilrs values (before normalization)
        let raw_lx = gamepad.value(Axis::LeftStickX);
        let raw_ly = gamepad.value(Axis::LeftStickY);
        controller.left_stick.raw_x = None;
        controller.left_stick.raw_y = None;
        controller.left_stick.gilrs_raw_x = Some(raw_lx);
        controller.left_stick.gilrs_raw_y = Some(raw_ly);
        let (lx, ly) = normalize_gilrs_stick(raw_lx, raw_ly);
        controller.left_stick.normalized_x = lx;
        controller.left_stick.normalized_y = ly;

        // Store raw gilrs values (before normalization)
        let raw_rx = gamepad.value(Axis::RightStickX);
        let raw_ry = gamepad.value(Axis::RightStickY);
        controller.right_stick.raw_x = None;
        controller.right_stick.raw_y = None;
        controller.right_stick.gilrs_raw_x = Some(raw_rx);
        controller.right_stick.gilrs_raw_y = Some(raw_ry);
        let (rx, ry) = normalize_gilrs_stick(raw_rx, raw_ry);
        controller.right_stick.normalized_x = rx;
        controller.right_stick.normalized_y = ry;

        // Triggers: check both axis (analog) and button (digital)
        // Some controllers have analog triggers (LeftZ/RightZ), others have digital (LeftTrigger2/RightTrigger2)
        use gilrs::Button;
        controller.left_trigger.raw = None;
        let lt_axis = (gamepad.value(Axis::LeftZ) + 1.0) / 2.0;
        let lt_button = if gamepad.is_pressed(Button::LeftTrigger2) {
            1.0
        } else {
            0.0
        };
        controller.left_trigger.normalized = lt_axis.max(lt_button);

        controller.right_trigger.raw = None;
        let rt_axis = (gamepad.value(Axis::RightZ) + 1.0) / 2.0;
        let rt_button = if gamepad.is_pressed(Button::RightTrigger2) {
            1.0
        } else {
            0.0
        };
        controller.right_trigger.normalized = rt_axis.max(rt_button);
        controller.buttons.update_from_gilrs(gamepad, capture);
    }

    /// Mark XInput controller as disconnected
    pub fn mark_xinput_disconnected(&mut self, user_index: u32) {
        if let Some(controller) = self.xinput_controllers.get_mut(user_index as usize) {
            controller.connected = false;
        }
    }

    /// Remove a gilrs controller (on disconnect)
    pub fn remove_gilrs_controller(&mut self, gamepad_id: GamepadId) {
        self.gilrs_controllers.remove(&gamepad_id);
    }

    /// Clear all stick trails for all controllers
    pub fn clear_all_trails(&mut self) {
        for controller in &mut self.xinput_controllers {
            controller.clear_trails();
        }
        for controller in self.gilrs_controllers.values_mut() {
            controller.clear_trails();
        }
    }
}

impl Default for VisualizerState {
    fn default() -> Self {
        Self::new()
    }
}

/// State for a single controller
#[derive(Debug)]
pub struct ControllerState {
    /// Backend-specific identification
    pub backend: ControllerBackend,
    pub connected: bool,
    pub left_stick: StickState,
    pub right_stick: StickState,
    pub left_trigger: TriggerState,
    pub right_trigger: TriggerState,
    pub buttons: ButtonStates,
    pub last_update: Instant,
    /// Trail for left stick visualization
    pub left_stick_trail: StickTrail,
    /// Trail for right stick visualization
    pub right_stick_trail: StickTrail,
}

impl ControllerState {
    /// Create a new controller state with the given backend
    pub fn new(backend: ControllerBackend) -> Self {
        Self {
            backend,
            connected: false,
            left_stick: StickState::default(),
            right_stick: StickState::default(),
            left_trigger: TriggerState::default(),
            right_trigger: TriggerState::default(),
            buttons: ButtonStates::default(),
            last_update: Instant::now(),
            left_stick_trail: StickTrail::default(),
            right_stick_trail: StickTrail::default(),
        }
    }

    /// Clear all stick trails for this controller
    pub fn clear_trails(&mut self) {
        self.left_stick_trail.clear();
        self.right_stick_trail.clear();
    }
}

/// Stick state (optional raw and normalized)
#[derive(Debug, Default, Clone, Copy)]
pub struct StickState {
    /// Raw X-axis (XInput only, None for gilrs)
    pub raw_x: Option<i16>,
    /// Raw Y-axis (XInput only, None for gilrs)
    pub raw_y: Option<i16>,
    pub normalized_x: f32,
    pub normalized_y: f32,
    /// Raw floating-point X (gilrs only, before normalization)
    pub gilrs_raw_x: Option<f32>,
    /// Raw floating-point Y (gilrs only, before normalization)
    pub gilrs_raw_y: Option<f32>,
}

/// Trigger state (optional raw and normalized)
#[derive(Debug, Default, Clone, Copy)]
pub struct TriggerState {
    /// Raw trigger value (XInput only, None for gilrs)
    pub raw: Option<u8>,
    pub normalized: f32,
}

/// Button states
#[derive(Debug, Default)]
pub struct ButtonStates {
    pub dpad_up: bool,
    pub dpad_down: bool,
    pub dpad_left: bool,
    pub dpad_right: bool,
    pub start: bool,
    pub back: bool,
    pub left_thumb: bool,
    pub right_thumb: bool,
    pub left_shoulder: bool,
    pub right_shoulder: bool,
    pub a: bool,
    pub b: bool,
    pub x: bool,
    pub y: bool,
    // Additional buttons (gilrs only)
    pub home: bool,
    pub capture: bool,
}

impl ButtonStates {
    /// Update from XInput raw button bitfield
    pub fn update_from_xinput(&mut self, b: u16) {
        // XInput button bit flags (from XInput API spec)
        self.dpad_up = b & 0x0001 != 0;
        self.dpad_down = b & 0x0002 != 0;
        self.dpad_left = b & 0x0004 != 0;
        self.dpad_right = b & 0x0008 != 0;
        self.start = b & 0x0010 != 0;
        self.back = b & 0x0020 != 0;
        self.left_thumb = b & 0x0040 != 0;
        self.right_thumb = b & 0x0080 != 0;
        self.left_shoulder = b & 0x0100 != 0;
        self.right_shoulder = b & 0x0200 != 0;
        self.a = b & 0x1000 != 0;
        self.b = b & 0x2000 != 0;
        self.x = b & 0x4000 != 0;
        self.y = b & 0x8000 != 0;
        // XInput doesn't have Home/Capture buttons (Guide button not exposed)
        self.home = false;
        self.capture = false;
    }

    /// Update from gilrs gamepad state
    ///
    /// Uses Nintendo-style button mapping (consistent with `super::buttons` module):
    /// - A = East (right), B = South (bottom), X = North (top), Y = West (left)
    ///
    /// # Arguments
    /// * `gamepad` - The gilrs gamepad
    /// * `capture` - Capture button state (tracked from raw events, not in gilrs mapping)
    pub fn update_from_gilrs(&mut self, gamepad: &gilrs::Gamepad, capture: bool) {
        use gilrs::Button;
        // Nintendo-style face button mapping (see buttons.rs for canonical mapping)
        // East=A (right), South=B (bottom), North=X (top), West=Y (left)
        self.a = gamepad.is_pressed(Button::East);
        self.b = gamepad.is_pressed(Button::South);
        self.x = gamepad.is_pressed(Button::North);
        self.y = gamepad.is_pressed(Button::West);
        // Shoulder buttons (LB/RB)
        self.left_shoulder = gamepad.is_pressed(Button::LeftTrigger);
        self.right_shoulder = gamepad.is_pressed(Button::RightTrigger);
        // Menu buttons
        self.back = gamepad.is_pressed(Button::Select);
        self.start = gamepad.is_pressed(Button::Start);
        // Stick clicks
        self.left_thumb = gamepad.is_pressed(Button::LeftThumb);
        self.right_thumb = gamepad.is_pressed(Button::RightThumb);
        // D-Pad
        self.dpad_up = gamepad.is_pressed(Button::DPadUp);
        self.dpad_down = gamepad.is_pressed(Button::DPadDown);
        self.dpad_left = gamepad.is_pressed(Button::DPadLeft);
        self.dpad_right = gamepad.is_pressed(Button::DPadRight);
        // Additional buttons
        self.home = gamepad.is_pressed(Button::Mode);
        self.capture = capture; // Tracked from raw button events
    }

    /// Get all buttons as an iterator of (name, pressed) pairs
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, bool)> {
        [
            ("A", self.a),
            ("B", self.b),
            ("X", self.x),
            ("Y", self.y),
            ("LB", self.left_shoulder),
            ("RB", self.right_shoulder),
            ("Back", self.back),
            ("Start", self.start),
            ("L3", self.left_thumb),
            ("R3", self.right_thumb),
            ("D-Up", self.dpad_up),
            ("D-Down", self.dpad_down),
            ("D-Left", self.dpad_left),
            ("D-Right", self.dpad_right),
            ("Home", self.home),
            ("Capture", self.capture),
        ]
        .into_iter()
    }
}
