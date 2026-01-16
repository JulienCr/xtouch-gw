import streamDeck, {
  DidReceiveSettingsEvent,
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
  renderDisconnectedImage,
  renderNotConfiguredImage,
  renderFlashImage,
} from "../services/button-renderer";

import type { JsonValue } from "@elgato/streamdeck";

/**
 * Base settings type with required server address and optional camera ID.
 * Extend this for action-specific settings.
 */
export interface BaseSettings {
  serverAddress: string;
  cameraId: string;
  [key: string]: JsonValue;
}

/**
 * Base context state for tracking individual button instances.
 * Extend this for action-specific state.
 */
export interface BaseContextState<TSettings extends BaseSettings> {
  settings: TSettings;
  client: XTouchClient | null;
  keyAction: KeyAction<TSettings>;
  connectionStatus: ConnectionStatus;
}

/**
 * Configuration for the blink animation on key press.
 */
const BLINK_CONFIG = {
  ITERATIONS: 2,
  DURATION_MS: 100,
} as const;

/**
 * Execute a yellow blink animation on a key action.
 * Used for visual feedback after successful operations.
 *
 * @param keyAction The Stream Deck key action
 * @param restoreDisplay Callback to restore the normal display after blinking
 */
export async function executeBlinkAnimation(
  keyAction: KeyAction<BaseSettings>,
  restoreDisplay: () => Promise<void>
): Promise<void> {
  const flashImage = renderFlashImage();

  for (let i = 0; i < BLINK_CONFIG.ITERATIONS; i++) {
    await keyAction.setImage(flashImage);
    await new Promise((resolve) => setTimeout(resolve, BLINK_CONFIG.DURATION_MS));
    await restoreDisplay();
    await new Promise((resolve) => setTimeout(resolve, BLINK_CONFIG.DURATION_MS));
  }
}

/**
 * Abstract base class for camera-related Stream Deck actions.
 * Provides common functionality for connection management, display updates, and state tracking.
 *
 * Subclasses must implement:
 * - normalizeSettings: Provide defaults for missing settings
 * - createContextState: Create the initial context state
 * - renderImage: Render the button image for current state
 * - getFallbackTitle: Get fallback title when image rendering fails
 * - setupClientCallbacks: Configure callbacks on the XTouch client
 *
 * Subclasses may override:
 * - updateStateFromClient: Update action-specific state from client
 */
export abstract class CameraActionBase<
  TSettings extends BaseSettings,
  TContextState extends BaseContextState<TSettings>
> extends SingletonAction<TSettings> {
  /**
   * Map of context IDs to their state.
   * Each Stream Deck button instance has a unique context ID.
   */
  protected contexts: Map<string, TContextState> = new Map();

  /**
   * Normalize settings by providing defaults for missing values.
   */
  protected abstract normalizeSettings(settings: TSettings): TSettings;

  /**
   * Create the initial context state for a new button instance.
   */
  protected abstract createContextState(
    settings: TSettings,
    keyAction: KeyAction<TSettings>
  ): TContextState;

  /**
   * Render the button image for the current state.
   * @returns Base64 data URL of the rendered image
   */
  protected abstract renderImage(contextState: TContextState): string;

  /**
   * Get fallback title when image rendering fails.
   */
  protected abstract getFallbackTitle(contextState: TContextState): string;

  /**
   * Set up client callbacks for state and connection changes.
   * Called after connecting to a new client.
   */
  protected abstract setupClientCallbacks(client: XTouchClient, serverAddress: string): void;

  /**
   * Update action-specific state from the current client.
   * Override in subclasses that track additional state beyond connection status.
   * Base implementation only updates connection status.
   */
  protected updateStateFromClient(contextState: TContextState): void {
    const { client } = contextState;
    if (!client) {
      contextState.connectionStatus = "disconnected";
      return;
    }
    contextState.connectionStatus = client.connectionStatus;
  }

  /**
   * Called when the action appears on the Stream Deck.
   * Initializes the context state and connects to the server.
   */
  override async onWillAppear(ev: WillAppearEvent<TSettings>): Promise<void> {
    const contextId = ev.action.id;
    const settings = ev.payload.settings;

    streamDeck.logger.info(`Action appeared: context=${contextId}, settings=${JSON.stringify(settings)}`);

    if (!ev.action.isKey()) {
      streamDeck.logger.error(`Action is not a key action: context=${contextId}`);
      return;
    }

    const keyAction = ev.action;
    const normalizedSettings = this.normalizeSettings(settings);
    const contextState = this.createContextState(normalizedSettings, keyAction);

    this.contexts.set(contextId, contextState);

    if (normalizedSettings.serverAddress) {
      this.connectContext(contextId);
    }

    await this.updateDisplay(contextState);
  }

  /**
   * Called when the action disappears from the Stream Deck.
   * Cleans up resources.
   */
  override async onWillDisappear(ev: WillDisappearEvent<TSettings>): Promise<void> {
    const contextId = ev.action.id;
    streamDeck.logger.info(`Action disappeared: context=${contextId}`);
    this.contexts.delete(contextId);
  }

  /**
   * Called when settings are received from the property inspector.
   * Updates stored settings and reconnects if necessary.
   */
  override async onDidReceiveSettings(ev: DidReceiveSettingsEvent<TSettings>): Promise<void> {
    const contextId = ev.action.id;
    const newSettings = ev.payload.settings;

    streamDeck.logger.info(`Settings received: context=${contextId}, settings=${JSON.stringify(newSettings)}`);

    const contextState = this.contexts.get(contextId);
    if (!contextState) {
      streamDeck.logger.warn(`No context state for ${contextId}`);
      return;
    }

    const oldServerAddress = contextState.settings.serverAddress;
    contextState.settings = this.normalizeSettings(newSettings);

    if (newSettings.serverAddress !== oldServerAddress) {
      streamDeck.logger.info(`Server address changed: ${oldServerAddress} -> ${newSettings.serverAddress}`);
      this.disconnectContextClient(contextState);

      if (newSettings.serverAddress) {
        this.connectContext(contextId);
      }
    }

    this.updateStateFromClient(contextState);
    await this.updateDisplay(contextState);
  }

  /**
   * Disconnect the client from a context (clears callbacks only).
   */
  protected disconnectContextClient(contextState: TContextState): void {
    if (contextState.client) {
      contextState.client.onStateChange = null;
      contextState.client.onConnectionChange = null;
      contextState.client = null;
    }
  }

  /**
   * Connect a context to the XTouch GW server.
   * Uses the shared client via getClient().
   */
  protected connectContext(contextId: string): void {
    const contextState = this.contexts.get(contextId);
    if (!contextState) return;

    const { serverAddress } = contextState.settings;
    if (!serverAddress) return;

    streamDeck.logger.info(`Connecting context ${contextId} to ${serverAddress}`);

    const client = getClient(serverAddress);
    contextState.client = client;

    this.setupClientCallbacks(client, serverAddress);

    if (client.connectionStatus === "disconnected") {
      client.connect();
    }

    this.updateStateFromClient(contextState);
    void this.updateDisplay(contextState);
  }

  /**
   * Handle connection status changes.
   * Updates all contexts connected to the given server.
   */
  protected handleConnectionChange(status: ConnectionStatus, serverAddress: string): void {
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
   * Update the action display based on current state.
   * Renders button images showing camera name and connection state.
   */
  protected async updateDisplay(contextState: TContextState): Promise<void> {
    const { keyAction, connectionStatus } = contextState;

    if (connectionStatus === "connecting") {
      await keyAction.setTitle("...");
      return;
    }

    try {
      const imageDataUrl = this.getDisplayImage(contextState);
      await keyAction.setTitle("");
      await keyAction.setImage(imageDataUrl);
    } catch (error) {
      streamDeck.logger.warn(`Failed to render button image, using title fallback: ${error}`);
      const title = this.getFallbackTitle(contextState);
      try {
        await keyAction.setTitle(title);
      } catch (fallbackError) {
        streamDeck.logger.debug(`Failed to update display in fallback: ${fallbackError}`);
      }
    }
  }

  /**
   * Get the appropriate display image based on connection status and configuration.
   */
  protected getDisplayImage(contextState: TContextState): string {
    const { connectionStatus, settings } = contextState;

    if (connectionStatus === "disconnected") {
      return renderDisconnectedImage();
    }
    if (!settings.cameraId) {
      return renderNotConfiguredImage();
    }
    return this.renderImage(contextState);
  }

  /**
   * Execute a camera reset with validation and visual feedback.
   * Returns true if reset was executed, false if validation failed.
   */
  protected async executeCameraReset(
    contextState: TContextState,
    resetMode: "position" | "zoom" | "both",
    keyAction: KeyAction<BaseSettings>
  ): Promise<boolean> {
    const { settings, client } = contextState;

    if (!settings.serverAddress || !settings.cameraId) {
      streamDeck.logger.warn("Camera reset: action not configured");
      await keyAction.showAlert();
      return false;
    }

    if (!client || client.connectionStatus !== "connected") {
      streamDeck.logger.warn("Camera reset: not connected to server");
      await keyAction.showAlert();
      return false;
    }

    try {
      await client.resetCamera(settings.cameraId, resetMode);
      streamDeck.logger.info(`Camera reset successful: ${settings.cameraId} (mode=${resetMode})`);

      await executeBlinkAnimation(keyAction, () => this.updateDisplay(contextState));
      return true;
    } catch (error) {
      streamDeck.logger.error(`Failed to reset camera: ${error}`);
      await keyAction.showAlert();
      return false;
    }
  }
}
