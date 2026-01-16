import streamDeck, {
  action,
  KeyDownEvent,
  KeyAction,
} from "@elgato/streamdeck";

import {
  CameraActionBase,
  BaseContextState,
  BaseSettings,
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

    streamDeck.logger.info(
      `Camera Reset key pressed: context=${contextId}, camera=${contextState.settings.cameraId}, mode=${contextState.settings.resetMode}`
    );

    await this.executeCameraReset(contextState, contextState.settings.resetMode, ev.action);
  }
}
