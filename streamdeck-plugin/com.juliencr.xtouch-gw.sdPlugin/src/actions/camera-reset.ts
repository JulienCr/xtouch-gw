import streamDeck, {
  action,
  KeyDownEvent,
  KeyAction,
} from "@elgato/streamdeck";

import {
  CameraActionBase,
  BaseContextState,
  BaseSettings,
  executeBlinkAnimation,
} from "./action-base";

import { XTouchClient, ConnectionStatus } from "../services/xtouch-client";
import { renderResetButtonImage } from "../services/button-renderer";

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
 */
interface CameraResetSettings extends BaseSettings {
  resetMode: ResetMode;
  [key: string]: JsonValue;
}

/**
 * Context state for the Camera Reset action.
 */
interface CameraResetContextState extends BaseContextState<CameraResetSettings> {}

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
export class CameraResetAction extends CameraActionBase<CameraResetSettings, CameraResetContextState> {
  protected override normalizeSettings(settings: CameraResetSettings): CameraResetSettings {
    return {
      serverAddress: settings.serverAddress || "",
      cameraId: settings.cameraId || "",
      resetMode: settings.resetMode || "both",
    };
  }

  protected override createContextState(
    settings: CameraResetSettings,
    keyAction: KeyAction<CameraResetSettings>
  ): CameraResetContextState {
    return {
      settings,
      client: null,
      keyAction,
      connectionStatus: "disconnected",
    };
  }

  protected override renderImage(contextState: CameraResetContextState): string {
    return renderResetButtonImage({ cameraId: contextState.settings.cameraId });
  }

  protected override getFallbackTitle(contextState: CameraResetContextState): string {
    const { connectionStatus, settings } = contextState;

    if (connectionStatus === "disconnected") {
      return "!";
    }
    if (!settings.cameraId) {
      return "Config";
    }
    return settings.cameraId;
  }

  protected override setupClientCallbacks(client: XTouchClient, serverAddress: string): void {
    client.onConnectionChange = (status: ConnectionStatus) => {
      this.handleConnectionChange(status, serverAddress);
    };
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

    if (!settings.serverAddress || !settings.cameraId) {
      streamDeck.logger.warn("Camera Reset action not configured");
      await ev.action.showAlert();
      return;
    }

    if (!client || client.connectionStatus !== "connected") {
      streamDeck.logger.warn("Not connected to XTouch GW server");
      await ev.action.showAlert();
      return;
    }

    try {
      await client.resetCamera(settings.cameraId, settings.resetMode);
      streamDeck.logger.info(`Camera reset successful: ${settings.cameraId} (mode=${settings.resetMode})`);

      await executeBlinkAnimation(
        ev.action as KeyAction<BaseSettings>,
        () => this.updateDisplay(contextState)
      );
    } catch (error) {
      streamDeck.logger.error(`Failed to reset camera: ${error}`);
      await ev.action.showAlert();
    }
  }
}
