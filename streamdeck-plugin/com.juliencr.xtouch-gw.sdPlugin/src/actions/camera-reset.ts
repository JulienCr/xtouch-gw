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
  ConnectionStatus,
  getClient,
} from "../services/xtouch-client";

import {
  renderResetButtonImage,
  renderDisconnectedImage,
  renderNotConfiguredImage,
  renderFlashImage,
} from "../services/button-renderer";

import type { JsonValue } from "@elgato/streamdeck";

/**
 * Reset mode for camera reset action.
 * - "position": Reset camera position (pan/tilt)
 * - "zoom": Reset camera zoom
 * - "both": Reset both position and zoom
 */
type ResetMode = "position" | "zoom" | "both";

/**
 * Settings for the Camera Reset action.
 * Index signature required to satisfy JsonObject constraint from SDK.
 */
type CameraResetSettings = {
  /** XTouch GW server address (host:port) */
  serverAddress: string;
  /** Camera ID to reset */
  cameraId: string;
  /** Reset mode: position, zoom, or both */
  resetMode: ResetMode;
  /** Index signature for JsonObject compatibility */
  [key: string]: JsonValue;
};

/**
 * Normalize settings by providing defaults for missing values.
 */
function normalizeSettings(settings: CameraResetSettings): CameraResetSettings {
  return {
    serverAddress: settings.serverAddress || "",
    cameraId: settings.cameraId || "",
    resetMode: settings.resetMode || "both",
  };
}

/**
 * Context state for tracking individual button instances
 */
interface ContextState {
  /** Current settings for this context */
  settings: CameraResetSettings;
  /** Reference to the XTouch client */
  client: XTouchClient | null;
  /** Reference to the KeyAction for this context */
  keyAction: KeyAction<CameraResetSettings>;
  /** Current connection status */
  connectionStatus: ConnectionStatus;
}

/**
 * Action that resets a camera's zoom and/or position.
 * When pressed, this action sends a reset request to the XTouch GW server.
 *
 * Features:
 * - Per-context state tracking for multiple buttons
 * - Real-time connection status via WebSocket
 * - Visual feedback showing camera name and connection state
 * - Shared client connections across contexts with same server
 */
@action({ UUID: "com.juliencr.xtouch-gw.camera-reset" })
export class CameraResetAction extends SingletonAction<CameraResetSettings> {
  /**
   * Map of context IDs to their state.
   * Each Stream Deck button instance has a unique context ID.
   */
  private contexts: Map<string, ContextState> = new Map();

  /**
   * Called when the action appears on the Stream Deck.
   * Initializes the context state and connects to the server.
   */
  override async onWillAppear(ev: WillAppearEvent<CameraResetSettings>): Promise<void> {
    const contextId = ev.action.id;
    const settings = ev.payload.settings;

    streamDeck.logger.info(`Camera Reset action appeared: context=${contextId}, settings=${JSON.stringify(settings)}`);

    // Ensure this is a key action (not a dial)
    if (!ev.action.isKey()) {
      streamDeck.logger.error(`Camera Reset action is not a key action: context=${contextId}`);
      return;
    }

    const keyAction = ev.action;

    // Initialize context state
    const contextState: ContextState = {
      settings: normalizeSettings(settings),
      client: null,
      keyAction,
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
  override async onWillDisappear(ev: WillDisappearEvent<CameraResetSettings>): Promise<void> {
    const contextId = ev.action.id;
    streamDeck.logger.info(`Camera Reset action disappeared: context=${contextId}`);

    // Remove context (client disconnection is handled separately via disconnectClient if needed)
    this.contexts.delete(contextId);
  }

  /**
   * Called when the key is pressed.
   * Sends the camera reset request to the server.
   */
  override async onKeyDown(ev: KeyDownEvent<CameraResetSettings>): Promise<void> {
    const contextId = ev.action.id;
    const contextState = this.contexts.get(contextId);

    if (!contextState) {
      streamDeck.logger.warn(`No context state for ${contextId}`);
      await ev.action.showAlert();
      return;
    }

    const { settings, client } = contextState;

    streamDeck.logger.info(
      `Camera Reset key pressed: context=${contextId}, camera=${settings.cameraId}, mode=${settings.resetMode}`
    );

    // Validate settings
    if (!settings.serverAddress || !settings.cameraId) {
      streamDeck.logger.warn("Camera Reset action not configured");
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
      // Make direct HTTP POST call to reset the camera
      const url = `http://${settings.serverAddress}/api/cameras/${encodeURIComponent(settings.cameraId)}/reset`;

      const response = await fetch(url, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          mode: settings.resetMode,
        }),
      });

      if (!response.ok) {
        const errorText = await response.text();
        throw new Error(`HTTP ${response.status} - ${errorText}`);
      }

      streamDeck.logger.info(`Camera reset successful: ${settings.cameraId} (mode=${settings.resetMode})`);

      // Yellow blink feedback (2 iterations)
      const flashImage = renderFlashImage();
      const BLINK_DURATION_MS = 100;

      for (let i = 0; i < 2; i++) {
        await ev.action.setImage(flashImage);
        await new Promise((resolve) => setTimeout(resolve, BLINK_DURATION_MS));
        await this.updateDisplay(contextState);
        await new Promise((resolve) => setTimeout(resolve, BLINK_DURATION_MS));
      }
    } catch (error) {
      streamDeck.logger.error(`Failed to reset camera: ${error}`);
      await ev.action.showAlert();
    }
  }

  /**
   * Called when settings are received from the property inspector.
   * Updates stored settings and reconnects if necessary.
   */
  override async onDidReceiveSettings(ev: DidReceiveSettingsEvent<CameraResetSettings>): Promise<void> {
    const contextId = ev.action.id;
    const newSettings = ev.payload.settings;

    streamDeck.logger.info(`Camera Reset settings received: context=${contextId}, settings=${JSON.stringify(newSettings)}`);

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
        contextState.client.onConnectionChange = null;
        contextState.client = null;
      }

      // Connect to new server
      if (newSettings.serverAddress) {
        this.connectContext(contextId);
      }
    }

    // Update display with current state
    this.updateConnectionFromClient(contextState);
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

    // Set up connection change callback
    client.onConnectionChange = (status: ConnectionStatus) => {
      this.handleConnectionChange(status, serverAddress);
    };

    // Start connection if not already connected
    if (client.connectionStatus === "disconnected") {
      client.connect();
    }

    // Update state from current client state
    this.updateConnectionFromClient(contextState);
    void this.updateDisplay(contextState);
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
   * Update context connection status from current client state.
   */
  private updateConnectionFromClient(contextState: ContextState): void {
    const { client } = contextState;
    if (!client) {
      contextState.connectionStatus = "disconnected";
      return;
    }

    contextState.connectionStatus = client.connectionStatus;
  }

  /**
   * Update the action display based on current state.
   * Renders button images showing camera name and connection state.
   */
  private async updateDisplay(contextState: ContextState): Promise<void> {
    const { settings, keyAction, connectionStatus } = contextState;

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
        // Render reset button with camera name and reset icon
        imageDataUrl = renderResetButtonImage({
          cameraId: settings.cameraId,
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
        await keyAction.setTitle(title);
      } catch (fallbackError) {
        // Action may have been removed, log but don't throw
        streamDeck.logger.debug(`Failed to update display in fallback: ${fallbackError}`);
      }
    }
  }
}
