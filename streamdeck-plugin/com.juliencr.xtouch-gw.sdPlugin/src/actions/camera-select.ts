import streamDeck, {
  action,
  DidReceiveSettingsEvent,
  KeyDownEvent,
  SingletonAction,
  WillAppearEvent,
  WillDisappearEvent,
  KeyAction,
} from "@elgato/streamdeck";

import {
  XTouchClient,
  XTouchState,
  ConnectionStatus,
  getClient,
} from "../services/xtouch-client";

import {
  renderButtonImage,
  renderDisconnectedImage,
  renderNotConfiguredImage,
} from "../services/button-renderer";

import type { JsonValue } from "@elgato/streamdeck";

/**
 * Settings for the Camera Select action.
 * Index signature required to satisfy JsonObject constraint from SDK.
 */
type CameraSelectSettings = {
  /** XTouch GW server address (host:port) */
  serverAddress: string;
  /** Gamepad slot identifier */
  gamepadSlot: string;
  /** Camera ID to target */
  cameraId: string;
  /** Index signature for JsonObject compatibility */
  [key: string]: JsonValue;
};

/**
 * Normalize settings by providing empty string defaults for missing values.
 */
function normalizeSettings(settings: CameraSelectSettings): CameraSelectSettings {
  return {
    serverAddress: settings.serverAddress || "",
    gamepadSlot: settings.gamepadSlot || "",
    cameraId: settings.cameraId || "",
  };
}

/**
 * Context state for tracking individual button instances
 */
interface ContextState {
  /** Current settings for this context */
  settings: CameraSelectSettings;
  /** Reference to the XTouch client */
  client: XTouchClient | null;
  /** Reference to the KeyAction for this context */
  keyAction: KeyAction<CameraSelectSettings>;
  /** Whether this camera is currently active (targeted) */
  isActive: boolean;
  /** Whether this camera is currently on air */
  isOnAir: boolean;
  /** Current connection status */
  connectionStatus: ConnectionStatus;
}

/**
 * Action that selects a camera for XTouch GW fader control.
 * When pressed, this action sends a camera target request to the XTouch GW server.
 *
 * Features:
 * - Per-context state tracking for multiple buttons
 * - Real-time state updates via WebSocket
 * - Visual feedback showing active/on-air status
 * - Shared client connections across contexts with same server
 */
@action({ UUID: "com.juliencr.xtouch-gw.camera-select" })
export class CameraSelectAction extends SingletonAction<CameraSelectSettings> {
  /**
   * Map of context IDs to their state.
   * Each Stream Deck button instance has a unique context ID.
   */
  private contexts: Map<string, ContextState> = new Map();

  /**
   * Called when the action appears on the Stream Deck.
   * Initializes the context state and connects to the server.
   */
  override async onWillAppear(ev: WillAppearEvent<CameraSelectSettings>): Promise<void> {
    const contextId = ev.action.id;
    const settings = ev.payload.settings;

    streamDeck.logger.info(`Camera Select action appeared: context=${contextId}, settings=${JSON.stringify(settings)}`);

    // Ensure this is a key action (not a dial)
    if (!ev.action.isKey()) {
      streamDeck.logger.error(`Camera Select action is not a key action: context=${contextId}`);
      return;
    }

    const keyAction = ev.action;

    // Initialize context state
    const contextState: ContextState = {
      settings: normalizeSettings(settings),
      client: null,
      keyAction,
      isActive: false,
      isOnAir: false,
      connectionStatus: "disconnected",
    };

    this.contexts.set(contextId, contextState);

    // Connect to server if configured
    if (contextState.settings.serverAddress) {
      this.connectContext(contextId);
    }

    // Update display
    await this.updateDisplay(contextState);
  }

  /**
   * Called when the action disappears from the Stream Deck.
   * Cleans up resources.
   */
  override async onWillDisappear(ev: WillDisappearEvent<CameraSelectSettings>): Promise<void> {
    const contextId = ev.action.id;
    streamDeck.logger.info(`Camera Select action disappeared: context=${contextId}`);

    // Remove context (client disconnection is handled separately via disconnectClient if needed)
    this.contexts.delete(contextId);
  }

  /**
   * Called when the key is pressed.
   * Sends the camera target request to the server.
   */
  override async onKeyDown(ev: KeyDownEvent<CameraSelectSettings>): Promise<void> {
    const contextId = ev.action.id;
    const contextState = this.contexts.get(contextId);

    if (!contextState) {
      streamDeck.logger.warn(`No context state for ${contextId}`);
      await ev.action.showAlert();
      return;
    }

    const { settings, client } = contextState;

    streamDeck.logger.info(
      `Camera Select key pressed: context=${contextId}, camera=${settings.cameraId}, slot=${settings.gamepadSlot}`
    );

    // Validate settings
    if (!settings.serverAddress || !settings.gamepadSlot || !settings.cameraId) {
      streamDeck.logger.warn("Camera Select action not configured");
      await ev.action.showAlert();
      return;
    }

    // Ensure client is connected
    if (!client || client.connectionStatus !== "connected") {
      streamDeck.logger.warn("Not connected to XTouch GW server");
      await ev.action.showAlert();
      return;
    }

    try {
      await client.setCameraTarget(settings.gamepadSlot, settings.cameraId);
      streamDeck.logger.info(`Camera target set successfully: ${settings.gamepadSlot} -> ${settings.cameraId}`);
      // Don't show the built-in green checkmark - the button image will update via state change
    } catch (error) {
      streamDeck.logger.error(`Failed to set camera target: ${error}`);
      await ev.action.showAlert();
    }
  }

  /**
   * Called when settings are received from the property inspector.
   * Updates stored settings and reconnects if necessary.
   */
  override async onDidReceiveSettings(ev: DidReceiveSettingsEvent<CameraSelectSettings>): Promise<void> {
    const contextId = ev.action.id;
    const newSettings = ev.payload.settings;

    streamDeck.logger.info(`Camera Select settings received: context=${contextId}, settings=${JSON.stringify(newSettings)}`);

    const contextState = this.contexts.get(contextId);
    if (!contextState) {
      streamDeck.logger.warn(`No context state for ${contextId}`);
      return;
    }

    const oldServerAddress = contextState.settings.serverAddress;

    // Update settings
    contextState.settings = normalizeSettings(newSettings);

    // Reconnect if server address changed
    if (newSettings.serverAddress !== oldServerAddress) {
      streamDeck.logger.info(`Server address changed: ${oldServerAddress} -> ${newSettings.serverAddress}`);

      // Disconnect old client callbacks
      if (contextState.client) {
        contextState.client.onStateChange = null;
        contextState.client.onConnectionChange = null;
        contextState.client = null;
      }

      // Connect to new server
      if (newSettings.serverAddress) {
        this.connectContext(contextId);
      }
    }

    // Update display with current state
    this.updateStateFromClient(contextState);
    await this.updateDisplay(contextState);
  }

  /**
   * Connect a context to the XTouch GW server.
   * Uses the shared client via getClient().
   */
  private connectContext(contextId: string): void {
    const contextState = this.contexts.get(contextId);
    if (!contextState) return;

    const { serverAddress } = contextState.settings;
    if (!serverAddress) return;

    streamDeck.logger.info(`Connecting context ${contextId} to ${serverAddress}`);

    // Get or create shared client
    const client = getClient(serverAddress);
    contextState.client = client;

    // Set up callbacks
    // Note: Multiple contexts may share the same client, so callbacks will update
    // all contexts that use this client when they're re-registered
    client.onStateChange = (state: XTouchState) => {
      this.handleStateChange(state);
    };

    client.onConnectionChange = (status: ConnectionStatus) => {
      this.handleConnectionChange(status, serverAddress);
    };

    // Start connection if not already connected
    if (client.connectionStatus === "disconnected") {
      client.connect();
    }

    // Update state from current client state
    this.updateStateFromClient(contextState);
    void this.updateDisplay(contextState);
  }

  /**
   * Handle state changes from the XTouch GW server.
   * Updates all contexts that match the changed state.
   */
  private handleStateChange(state: XTouchState): void {
    for (const [contextId, contextState] of this.contexts) {
      if (!contextState.client) continue;

      const { gamepadSlot, cameraId } = contextState.settings;
      const wasActive = contextState.isActive;
      const wasOnAir = contextState.isOnAir;

      // Update active state
      if (gamepadSlot && cameraId) {
        const gamepad = state.gamepads.get(gamepadSlot);
        contextState.isActive = gamepad?.current_camera === cameraId;
      } else {
        contextState.isActive = false;
      }

      // Update on-air state
      contextState.isOnAir = cameraId ? state.onAirCameraId === cameraId : false;

      // Only update display if state changed
      if (contextState.isActive !== wasActive || contextState.isOnAir !== wasOnAir) {
        streamDeck.logger.debug(
          `State changed for ${contextId}: active=${contextState.isActive}, onAir=${contextState.isOnAir}`
        );
        void this.updateDisplay(contextState);
      }
    }
  }

  /**
   * Handle connection status changes.
   * Updates all contexts connected to the given server.
   */
  private handleConnectionChange(status: ConnectionStatus, serverAddress: string): void {
    const normalizedAddress = serverAddress.toLowerCase().trim();

    for (const [contextId, contextState] of this.contexts) {
      if (contextState.settings.serverAddress.toLowerCase().trim() !== normalizedAddress) {
        continue;
      }

      if (contextState.connectionStatus !== status) {
        streamDeck.logger.info(
          `Connection status changed for ${contextId}: ${contextState.connectionStatus} -> ${status}`
        );
        contextState.connectionStatus = status;
        void this.updateDisplay(contextState);
      }
    }
  }

  /**
   * Update context state from current client state.
   */
  private updateStateFromClient(contextState: ContextState): void {
    const { client, settings } = contextState;
    if (!client) {
      contextState.connectionStatus = "disconnected";
      contextState.isActive = false;
      contextState.isOnAir = false;
      return;
    }

    contextState.connectionStatus = client.connectionStatus;

    if (client.connectionStatus === "connected") {
      const state = client.getState();
      const { gamepadSlot, cameraId } = settings;

      if (gamepadSlot && cameraId) {
        contextState.isActive = client.isControlledBy(cameraId, gamepadSlot);
      } else {
        contextState.isActive = false;
      }

      contextState.isOnAir = cameraId ? client.isOnAir(cameraId) : false;
    } else {
      contextState.isActive = false;
      contextState.isOnAir = false;
    }
  }

  /**
   * Update the action display based on current state.
   * Renders button images showing camera name, active status, and connection state.
   */
  private async updateDisplay(contextState: ContextState): Promise<void> {
    const { settings, keyAction, isActive, isOnAir, connectionStatus } = contextState;

    try {
      let imageDataUrl: string;

      if (connectionStatus === "disconnected") {
        // Show disconnected image with red "!" icon
        imageDataUrl = renderDisconnectedImage();
      } else if (connectionStatus === "connecting") {
        // Show connecting state - use text animation for now
        await keyAction.setTitle("...");
        return;
      } else if (!settings.cameraId) {
        // Show not configured image with gear icon
        imageDataUrl = renderNotConfiguredImage();
      } else {
        // Render button with current state
        // - isControlled: green background + bottom indicator bar
        // - isOnAir: red border
        imageDataUrl = renderButtonImage({
          cameraId: settings.cameraId,
          isControlled: isActive,
          isOnAir: isOnAir,
        });
      }

      // Clear title and set the rendered image
      await keyAction.setTitle("");
      await keyAction.setImage(imageDataUrl);
    } catch (error) {
      // Fallback to title-based display if rendering fails
      streamDeck.logger.warn(`Failed to render button image, using title fallback: ${error}`);

      let title: string;
      if (connectionStatus === "disconnected") {
        title = "!";
      } else if (!settings.cameraId) {
        title = "Config";
      } else {
        title = settings.cameraId;
      }

      try {
        if (isActive && isOnAir) {
          await keyAction.setTitle(`[LIVE]\n${title}`);
        } else if (isActive) {
          await keyAction.setTitle(`[*]\n${title}`);
        } else if (isOnAir) {
          await keyAction.setTitle(`(LIVE)\n${title}`);
        } else {
          await keyAction.setTitle(title);
        }
      } catch (fallbackError) {
        // Action may have been removed, log but don't throw
        streamDeck.logger.debug(`Failed to update display in fallback: ${fallbackError}`);
      }
    }
  }
}
