/**
 * Gamepad slot information from the XTouch GW API
 */
export interface GamepadSlotInfo {
  slot: string;
  product_match: string;
  camera_target_mode: string;
  current_camera: string | null;
}

/**
 * Camera information from the XTouch GW API
 */
export interface CameraInfo {
  id: string;
  scene: string;
  source: string;
  split_source: string;
  enable_ptz: boolean;
}

/**
 * Connection status for the WebSocket client
 */
export type ConnectionStatus = "disconnected" | "connecting" | "connected";

/**
 * Full state snapshot from the XTouch GW server
 */
export interface XTouchState {
  gamepads: Map<string, GamepadSlotInfo>;
  cameras: Map<string, CameraInfo>;
  onAirCameraId: string | null;
  connectionStatus: ConnectionStatus;
}

/**
 * Snapshot message received on WebSocket connect
 */
export interface SnapshotMessage {
  type: "snapshot";
  gamepads: GamepadSlotInfo[];
  cameras: CameraInfo[];
  on_air_camera: string | null;
  timestamp: number;
}

/**
 * Target changed message received when a gamepad's camera target changes
 */
export interface TargetChangedMessage {
  type: "target_changed";
  gamepad_slot: string;
  camera_id: string;
  timestamp: number;
}

/**
 * On air changed message received when OBS program scene changes
 */
export interface OnAirChangedMessage {
  type: "on_air_changed";
  camera_id: string;
  scene_name: string;
  timestamp: number;
}

/**
 * Union type for all WebSocket messages
 */
export type WebSocketMessage = SnapshotMessage | TargetChangedMessage | OnAirChangedMessage;
