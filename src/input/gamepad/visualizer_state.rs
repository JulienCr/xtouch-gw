//! State structures for gamepad visualizer
//!
//! Tracks raw and normalized values for all connected XInput controllers

use std::time::Instant;

/// State for all controllers being visualized
#[derive(Debug)]
pub struct VisualizerState {
    pub controllers: Vec<ControllerState>,
}

impl VisualizerState {
    /// Create new state with 4 controller slots (XInput user indices 0-3)
    pub fn new() -> Self {
        Self {
            controllers: (0..4).map(ControllerState::new).collect(),
        }
    }

    /// Update state from XInput gamepad state
    pub fn update_from_xinput(&mut self, user_index: u32, state: &rusty_xinput::XInputState) {
        if let Some(controller) = self.controllers.get_mut(user_index as usize) {
            controller.connected = true;
            controller.packet_number = state.raw.dwPacketNumber;
            controller.last_update = Instant::now();

            // Update sticks (raw values - access via state.raw.Gamepad)
            controller.left_stick.raw_x = state.raw.Gamepad.sThumbLX;
            controller.left_stick.raw_y = state.raw.Gamepad.sThumbLY;
            controller.right_stick.raw_x = state.raw.Gamepad.sThumbRX;
            controller.right_stick.raw_y = state.raw.Gamepad.sThumbRY;

            // Update triggers (raw values - use methods)
            controller.left_trigger.raw = state.left_trigger();
            controller.right_trigger.raw = state.right_trigger();

            // Update buttons (pass button bitfield)
            controller
                .buttons
                .update_from_xinput(state.raw.Gamepad.wButtons);
        }
    }

    /// Mark controller as disconnected
    pub fn mark_disconnected(&mut self, user_index: u32) {
        if let Some(controller) = self.controllers.get_mut(user_index as usize) {
            controller.connected = false;
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
    pub user_index: u32,
    pub connected: bool,
    pub packet_number: u32,

    // Sticks (raw i16 + normalized f32)
    pub left_stick: StickState,
    pub right_stick: StickState,

    // Triggers (raw u8 + normalized f32)
    pub left_trigger: TriggerState,
    pub right_trigger: TriggerState,

    // Buttons (bool state)
    pub buttons: ButtonStates,

    pub last_update: Instant,
}

impl ControllerState {
    fn new(user_index: u32) -> Self {
        Self {
            user_index,
            connected: false,
            packet_number: 0,
            left_stick: StickState::default(),
            right_stick: StickState::default(),
            left_trigger: TriggerState::default(),
            right_trigger: TriggerState::default(),
            buttons: ButtonStates::default(),
            last_update: Instant::now(),
        }
    }
}

/// Stick state (both raw and normalized)
#[derive(Debug, Default, Clone, Copy)]
pub struct StickState {
    pub raw_x: i16,
    pub raw_y: i16,
    pub normalized_x: f32,
    pub normalized_y: f32,
}

/// Trigger state (both raw and normalized)
#[derive(Debug, Default, Clone, Copy)]
pub struct TriggerState {
    pub raw: u8,
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
}

impl ButtonStates {
    /// Update from XInput raw button bitfield
    pub fn update_from_xinput(&mut self, buttons: u16) {
        // XInput button bit flags (from XInput API spec)
        const DPAD_UP: u16 = 0x0001;
        const DPAD_DOWN: u16 = 0x0002;
        const DPAD_LEFT: u16 = 0x0004;
        const DPAD_RIGHT: u16 = 0x0008;
        const START: u16 = 0x0010;
        const BACK: u16 = 0x0020;
        const LEFT_THUMB: u16 = 0x0040;
        const RIGHT_THUMB: u16 = 0x0080;
        const LEFT_SHOULDER: u16 = 0x0100;
        const RIGHT_SHOULDER: u16 = 0x0200;
        const A: u16 = 0x1000;
        const B: u16 = 0x2000;
        const X: u16 = 0x4000;
        const Y: u16 = 0x8000;

        self.dpad_up = (buttons & DPAD_UP) != 0;
        self.dpad_down = (buttons & DPAD_DOWN) != 0;
        self.dpad_left = (buttons & DPAD_LEFT) != 0;
        self.dpad_right = (buttons & DPAD_RIGHT) != 0;
        self.start = (buttons & START) != 0;
        self.back = (buttons & BACK) != 0;
        self.left_thumb = (buttons & LEFT_THUMB) != 0;
        self.right_thumb = (buttons & RIGHT_THUMB) != 0;
        self.left_shoulder = (buttons & LEFT_SHOULDER) != 0;
        self.right_shoulder = (buttons & RIGHT_SHOULDER) != 0;
        self.a = (buttons & A) != 0;
        self.b = (buttons & B) != 0;
        self.x = (buttons & X) != 0;
        self.y = (buttons & Y) != 0;
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
        ]
        .into_iter()
    }
}
