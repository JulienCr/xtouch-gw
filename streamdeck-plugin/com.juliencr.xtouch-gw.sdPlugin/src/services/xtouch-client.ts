import streamDeck from "@elgato/streamdeck";
import { WebSocket, type RawData } from "ws";

import type {
  GamepadSlotInfo,
  CameraInfo,
  ConnectionStatus,
  XTouchState,
  SnapshotMessage,
  TargetChangedMessage,
  OnAirChangedMessage,
  WebSocketMessage,
} from "./xtouch-types";

import { setCamera, resetCamera, getGamepads, getCameras } from "./xtouch-api";

// Re-export all types so existing imports from "../services/xtouch-client" still work
export type {
  GamepadSlotInfo,
  CameraInfo,
  ConnectionStatus,
  XTouchState,
  SnapshotMessage,
  TargetChangedMessage,
  OnAirChangedMessage,
  WebSocketMessage,
} from "./xtouch-types";

// Re-export HTTP API functions for consumers that import from this module
export { apiRequest, setCamera, resetCamera, getGamepads, getCameras } from "./xtouch-api";

/**
 * Client for communicating with the XTouch GW server.
 * Handles WebSocket connection for real-time state updates and HTTP API calls for camera targeting.
 */
export class XTouchClient {
  private _serverAddress: string;
  private _connectionStatus: ConnectionStatus = "disconnected";
  private _ws: WebSocket | null = null;
  private _reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private _reconnectAttempts: number = 0;
  private _shouldReconnect: boolean = false;

  // State
  private _gamepads: Map<string, GamepadSlotInfo> = new Map();
  private _cameras: Map<string, CameraInfo> = new Map();
  private _onAirCameraId: string | null = null;

  // Listener sets (supports multiple actions sharing one client)
  private _stateChangeListeners = new Set<(state: XTouchState) => void>();
  private _connectionChangeListeners = new Set<(status: ConnectionStatus) => void>();

  // Reconnect configuration
  private static readonly INITIAL_RECONNECT_DELAY_MS = 1000;
  private static readonly MAX_RECONNECT_DELAY_MS = 30000;

  // WebSocket close codes
  private static readonly CLOSE_NORMAL = 1000;

  constructor(serverAddress: string) {
    this._serverAddress = serverAddress;
  }

  /**
   * Get the server address
   */
  get serverAddress(): string {
    return this._serverAddress;
  }

  /**
   * Get the current connection status
   */
  get connectionStatus(): ConnectionStatus {
    return this._connectionStatus;
  }

  /**
   * Add a listener for state changes.
   */
  addStateChangeListener(callback: (state: XTouchState) => void): void {
    this._stateChangeListeners.add(callback);
  }

  /**
   * Remove a state change listener.
   */
  removeStateChangeListener(callback: (state: XTouchState) => void): void {
    this._stateChangeListeners.delete(callback);
  }

  /**
   * Add a listener for connection status changes.
   */
  addConnectionChangeListener(callback: (status: ConnectionStatus) => void): void {
    this._connectionChangeListeners.add(callback);
  }

  /**
   * Remove a connection change listener.
   */
  removeConnectionChangeListener(callback: (status: ConnectionStatus) => void): void {
    this._connectionChangeListeners.delete(callback);
  }

  /**
   * Check if this client has any registered listeners.
   */
  get hasListeners(): boolean {
    return this._stateChangeListeners.size > 0 || this._connectionChangeListeners.size > 0;
  }

  /**
   * Connect to the XTouch GW WebSocket server.
   * Automatically reconnects on disconnect with exponential backoff.
   */
  connect(): void {
    if (this._ws && this._connectionStatus !== "disconnected") {
      streamDeck.logger.info("Already connected or connecting, ignoring connect call");
      return;
    }

    this._shouldReconnect = true;
    this._reconnectAttempts = 0;
    this.doConnect();
  }

  /**
   * Internal connect method
   */
  private doConnect(): void {
    if (this._reconnectTimer) {
      clearTimeout(this._reconnectTimer);
      this._reconnectTimer = null;
    }

    this.setConnectionStatus("connecting");

    const wsUrl = `ws://${this._serverAddress}/api/ws/camera-updates`;
    streamDeck.logger.info(`Connecting to XTouch GW WebSocket at ${wsUrl}`);

    try {
      this._ws = new WebSocket(wsUrl);
      this.setupWebSocketHandlers();
    } catch (error) {
      streamDeck.logger.error(`Failed to create WebSocket: ${error}`);
      this.setConnectionStatus("disconnected");
      this.scheduleReconnect();
    }
  }

  /**
   * Set up WebSocket event handlers
   */
  private setupWebSocketHandlers(): void {
    if (!this._ws) return;

    this._ws.on("open", () => {
      streamDeck.logger.info("WebSocket connected to XTouch GW");
      this._reconnectAttempts = 0;
      this.setConnectionStatus("connected");
    });

    this._ws.on("close", (code: number, reason: Buffer) => {
      streamDeck.logger.info(`WebSocket closed: code=${code}, reason=${reason.toString()}`);
      this._ws = null;
      this.setConnectionStatus("disconnected");

      if (this._shouldReconnect) {
        this.scheduleReconnect();
      }
    });

    this._ws.on("error", (error: Error) => {
      streamDeck.logger.error(`WebSocket error: ${error.message}`);
      // onclose will be called after onerror, so we handle reconnection there
    });

    this._ws.on("message", (data: RawData) => {
      this.handleMessage(data.toString());
    });
  }

  /**
   * Handle incoming WebSocket message
   */
  private handleMessage(data: string): void {
    try {
      const message = JSON.parse(data) as WebSocketMessage;

      switch (message.type) {
        case "snapshot":
          this.handleSnapshot(message);
          break;
        case "target_changed":
          this.handleTargetChanged(message);
          break;
        case "on_air_changed":
          this.handleOnAirChanged(message);
          break;
        default:
          streamDeck.logger.warn(`Unknown message type: ${(message as { type: string }).type}`);
      }
    } catch (error) {
      streamDeck.logger.error(`Failed to parse WebSocket message: ${error}`);
    }
  }

  /**
   * Handle snapshot message (full state on connect)
   */
  private handleSnapshot(message: SnapshotMessage): void {
    streamDeck.logger.info(
      `Received snapshot: ${message.gamepads.length} gamepads, ${message.cameras.length} cameras`
    );

    // Update gamepads map
    this._gamepads.clear();
    for (const gamepad of message.gamepads) {
      this._gamepads.set(gamepad.slot, gamepad);
    }

    // Update cameras map
    this._cameras.clear();
    for (const camera of message.cameras) {
      this._cameras.set(camera.id, camera);
    }

    // Update on-air camera
    this._onAirCameraId = message.on_air_camera;

    this.emitStateChange();
  }

  /**
   * Handle target changed message
   */
  private handleTargetChanged(message: TargetChangedMessage): void {
    streamDeck.logger.info(
      `Camera target changed: ${message.gamepad_slot} -> ${message.camera_id}`
    );

    const gamepad = this._gamepads.get(message.gamepad_slot);
    if (gamepad) {
      // Object retrieved from Map is mutated directly; no need to re-set since Map still references the same instance
      gamepad.current_camera = message.camera_id;
    } else {
      // Create a placeholder entry if gamepad not found
      this._gamepads.set(message.gamepad_slot, {
        slot: message.gamepad_slot,
        product_match: "",
        camera_target_mode: "unknown",
        current_camera: message.camera_id,
      });
    }

    this.emitStateChange();
  }

  /**
   * Handle on-air changed message
   */
  private handleOnAirChanged(message: OnAirChangedMessage): void {
    streamDeck.logger.info(
      `On-air camera changed: ${message.camera_id} (scene: ${message.scene_name})`
    );

    this._onAirCameraId = message.camera_id;
    this.emitStateChange();
  }

  /**
   * Schedule a reconnection attempt with exponential backoff
   */
  private scheduleReconnect(): void {
    if (!this._shouldReconnect) return;

    const delay = Math.min(
      XTouchClient.INITIAL_RECONNECT_DELAY_MS * Math.pow(2, this._reconnectAttempts),
      XTouchClient.MAX_RECONNECT_DELAY_MS
    );

    this._reconnectAttempts++;

    streamDeck.logger.info(
      `Scheduling reconnect attempt ${this._reconnectAttempts} in ${delay}ms`
    );

    this._reconnectTimer = setTimeout(() => {
      this._reconnectTimer = null;
      if (this._shouldReconnect) {
        this.doConnect();
      }
    }, delay);
  }

  /**
   * Disconnect from the WebSocket server
   */
  disconnect(): void {
    streamDeck.logger.info("Disconnecting from XTouch GW");

    this._shouldReconnect = false;

    if (this._reconnectTimer) {
      clearTimeout(this._reconnectTimer);
      this._reconnectTimer = null;
    }

    if (this._ws) {
      this._ws.close(XTouchClient.CLOSE_NORMAL, "Client disconnecting");
      this._ws = null;
    }

    this.setConnectionStatus("disconnected");
  }

  /**
   * Set connection status and notify all listeners.
   */
  private setConnectionStatus(status: ConnectionStatus): void {
    if (this._connectionStatus === status) return;

    this._connectionStatus = status;

    for (const listener of this._connectionChangeListeners) {
      try {
        listener(status);
      } catch (error) {
        streamDeck.logger.error(`Error in connection change listener: ${error}`);
      }
    }
  }

  /**
   * Notify all state change listeners.
   * Builds the state snapshot once and shares it across all listeners.
   */
  private emitStateChange(): void {
    if (this._stateChangeListeners.size === 0) return;

    const state = this.getState();
    for (const listener of this._stateChangeListeners) {
      try {
        listener(state);
      } catch (error) {
        streamDeck.logger.error(`Error in state change listener: ${error}`);
      }
    }
  }

  /**
   * Get current state snapshot
   */
  getState(): XTouchState {
    return {
      gamepads: new Map(this._gamepads),
      cameras: new Map(this._cameras),
      onAirCameraId: this._onAirCameraId,
      connectionStatus: this._connectionStatus,
    };
  }

  /**
   * Check if a camera is currently controlled by a specific gamepad
   */
  isControlledBy(cameraId: string, gamepadSlot: string): boolean {
    const gamepad = this._gamepads.get(gamepadSlot);
    return gamepad?.current_camera === cameraId;
  }

  /**
   * Check if a camera is currently on air
   */
  isOnAir(cameraId: string): boolean {
    return this._onAirCameraId === cameraId;
  }

  /**
   * Set the camera target for a gamepad slot via HTTP API.
   *
   * @param slot The gamepad slot identifier
   * @param cameraId The camera ID to target
   * @param target Optional: "preview" or "program" to also switch OBS scene (default: "preview")
   */
  async setCameraTarget(slot: string, cameraId: string, target: "preview" | "program" = "preview"): Promise<void> {
    await setCamera(this._serverAddress, slot, cameraId, target);
  }

  /**
   * Fetch available gamepad slots via HTTP API.
   */
  async getGamepadSlots(): Promise<GamepadSlotInfo[]> {
    return getGamepads(this._serverAddress);
  }

  /**
   * Fetch available cameras via HTTP API.
   */
  async getCameras(): Promise<CameraInfo[]> {
    return getCameras(this._serverAddress);
  }

  /**
   * Reset a camera's zoom and/or position via HTTP API.
   */
  async resetCamera(cameraId: string, mode: "position" | "zoom" | "both"): Promise<void> {
    await resetCamera(this._serverAddress, cameraId, mode);
  }
}

/**
 * Client instances per server address (singleton pattern)
 */
const clientInstances: Map<string, XTouchClient> = new Map();

/**
 * Get or create a client instance for a server address.
 * Multiple actions can share the same client instance per server.
 *
 * @param serverAddress The server address (host:port)
 * @returns The XTouchClient instance for that server
 */
export function getClient(serverAddress: string): XTouchClient {
  const normalizedAddress = serverAddress.toLowerCase().trim();

  let client = clientInstances.get(normalizedAddress);
  if (!client) {
    client = new XTouchClient(normalizedAddress);
    clientInstances.set(normalizedAddress, client);
    streamDeck.logger.info(`Created new XTouchClient for ${normalizedAddress}`);
  }

  return client;
}

/**
 * Disconnect and remove a client instance.
 * Use this when the server address changes or plugin unloads.
 *
 * @param serverAddress The server address to disconnect
 */
export function disconnectClient(serverAddress: string): void {
  const normalizedAddress = serverAddress.toLowerCase().trim();

  const client = clientInstances.get(normalizedAddress);
  if (client) {
    client.disconnect();
    clientInstances.delete(normalizedAddress);
    streamDeck.logger.info(`Disconnected and removed XTouchClient for ${normalizedAddress}`);
  }
}

/**
 * Disconnect all client instances.
 * Use this on plugin shutdown.
 */
export function disconnectAllClients(): void {
  for (const [address, client] of clientInstances) {
    client.disconnect();
    streamDeck.logger.info(`Disconnected XTouchClient for ${address}`);
  }
  clientInstances.clear();
}
