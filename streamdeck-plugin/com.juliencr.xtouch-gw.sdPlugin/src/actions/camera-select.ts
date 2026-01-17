import streamDeck, {
  action,
  KeyDownEvent,
  KeyUpEvent,
  KeyAction,
  WillDisappearEvent,
} from "@elgato/streamdeck";

import {
  CameraActionBase,
  BaseContextState,
  BaseSettings,
} from "./action-base";

import {
  XTouchClient,
  XTouchState,
  ConnectionStatus,
} from "../services/xtouch-client";

import { renderButtonImage } from "../services/button-renderer";

import type { JsonValue } from "@elgato/streamdeck";

/**
 * Settings for the Camera Select action.
 */
interface CameraSelectSettings extends BaseSettings {
  gamepadSlot: string;
  resetMode: "position" | "zoom" | "both";
  [key: string]: JsonValue;
}

/**
 * Context state for the Camera Select action.
 */
interface CameraSelectContextState extends BaseContextState<CameraSelectSettings> {
  isActive: boolean;
  isOnAir: boolean;
  longPressTimer: ReturnType<typeof setTimeout> | null;
  longPressTriggered: boolean;
}

/**
 * Long press threshold in milliseconds.
 * Hold longer than this to trigger reset instead of select.
 */
const LONG_PRESS_THRESHOLD_MS = 500;

/**
 * Action that selects a camera for XTouch GW fader control.
 * When pressed, this action sends a camera target request to the XTouch GW server.
 *
 * Features:
 * - Per-context state tracking for multiple buttons
 * - Real-time state updates via WebSocket
 * - Visual feedback showing active/on-air status
 * - Shared client connections across contexts with same server
 * - Long press (500ms) triggers camera reset
 */
@action({ UUID: "com.juliencr.xtouch-gw.camera-select" })
export class CameraSelectAction extends CameraActionBase<CameraSelectSettings, CameraSelectContextState> {
  protected override normalizeSettings(settings: CameraSelectSettings): CameraSelectSettings {
    return {
      serverAddress: settings.serverAddress || "",
      gamepadSlot: settings.gamepadSlot || "",
      cameraId: settings.cameraId || "",
      resetMode: settings.resetMode || "both",
    };
  }

  protected override createContextState(
    settings: CameraSelectSettings,
    keyAction: KeyAction<CameraSelectSettings>
  ): CameraSelectContextState {
    return {
      settings,
      client: null,
      keyAction,
      isActive: false,
      isOnAir: false,
      connectionStatus: "disconnected",
      longPressTimer: null,
      longPressTriggered: false,
    };
  }

  protected override renderImage(contextState: CameraSelectContextState): string {
    return renderButtonImage({
      cameraId: contextState.settings.cameraId,
      isControlled: contextState.isActive,
      isOnAir: contextState.isOnAir,
    });
  }

  protected override getFallbackTitle(contextState: CameraSelectContextState): string {
    const { connectionStatus, settings, isActive, isOnAir } = contextState;

    if (connectionStatus === "disconnected") {
      return "!";
    }
    if (!settings.cameraId) {
      return "Config";
    }
    if (isActive && isOnAir) {
      return `[LIVE]\n${settings.cameraId}`;
    }
    if (isActive) {
      return `[*]\n${settings.cameraId}`;
    }
    if (isOnAir) {
      return `(LIVE)\n${settings.cameraId}`;
    }
    return settings.cameraId;
  }

  protected override setupClientCallbacks(client: XTouchClient, serverAddress: string): void {
    client.onStateChange = (state: XTouchState) => {
      this.handleStateChange(state);
    };
    client.onConnectionChange = (status: ConnectionStatus) => {
      this.handleConnectionChange(status, serverAddress);
    };
  }

  protected override updateStateFromClient(contextState: CameraSelectContextState): void {
    const { client, settings } = contextState;

    if (!client) {
      contextState.connectionStatus = "disconnected";
      contextState.isActive = false;
      contextState.isOnAir = false;
      return;
    }

    contextState.connectionStatus = client.connectionStatus;

    if (client.connectionStatus === "connected") {
      const { gamepadSlot, cameraId } = settings;
      contextState.isActive = gamepadSlot && cameraId
        ? client.isControlledBy(cameraId, gamepadSlot)
        : false;
      contextState.isOnAir = cameraId ? client.isOnAir(cameraId) : false;
    } else {
      contextState.isActive = false;
      contextState.isOnAir = false;
    }
  }

  /**
   * Called when the action disappears from the Stream Deck.
   * Clears the long press timer to prevent orphaned callbacks.
   */
  override async onWillDisappear(ev: WillDisappearEvent<CameraSelectSettings>): Promise<void> {
    const contextId = ev.action.id;
    const contextState = this.contexts.get(contextId);

    if (contextState?.longPressTimer) {
      clearTimeout(contextState.longPressTimer);
      contextState.longPressTimer = null;
    }

    await super.onWillDisappear(ev);
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

      if (gamepadSlot && cameraId) {
        const gamepad = state.gamepads.get(gamepadSlot);
        contextState.isActive = gamepad?.current_camera === cameraId;
      } else {
        contextState.isActive = false;
      }

      contextState.isOnAir = cameraId ? state.onAirCameraId === cameraId : false;

      if (contextState.isActive !== wasActive || contextState.isOnAir !== wasOnAir) {
        streamDeck.logger.debug(
          `State changed for ${contextId}: active=${contextState.isActive}, onAir=${contextState.isOnAir}`
        );
        void this.updateDisplay(contextState);
      }
    }
  }

  /**
   * Called when the key is pressed.
   * Starts a timer for long press detection - reset triggers automatically after 500ms.
   */
  override async onKeyDown(ev: KeyDownEvent<CameraSelectSettings>): Promise<void> {
    const contextId = ev.action.id;
    const contextState = this.contexts.get(contextId);
    if (!contextState) return;

    contextState.longPressTriggered = false;

    if (contextState.longPressTimer) {
      clearTimeout(contextState.longPressTimer);
    }

    contextState.longPressTimer = setTimeout(() => {
      contextState.longPressTimer = null;
      contextState.longPressTriggered = true;
      void this.executeReset(contextId, ev.action);
    }, LONG_PRESS_THRESHOLD_MS);
  }

  /**
   * Execute camera reset (called when long press timer fires).
   */
  private async executeReset(contextId: string, keyAction: KeyAction<CameraSelectSettings>): Promise<void> {
    const contextState = this.contexts.get(contextId);
    if (!contextState) return;

    streamDeck.logger.info(`Long press triggered - resetting camera ${contextState.settings.cameraId}`);
    await this.executeCameraReset(contextState, contextState.settings.resetMode || "both", keyAction);
  }

  /**
   * Called when the key is released.
   * If released before 500ms, cancels reset timer and executes camera select.
   * If reset already triggered, does nothing.
   */
  override async onKeyUp(ev: KeyUpEvent<CameraSelectSettings>): Promise<void> {
    const contextId = ev.action.id;
    const contextState = this.contexts.get(contextId);

    if (!contextState) {
      streamDeck.logger.warn(`No context state for ${contextId}`);
      await ev.action.showAlert();
      return;
    }

    if (contextState.longPressTimer) {
      clearTimeout(contextState.longPressTimer);
      contextState.longPressTimer = null;
    }

    if (contextState.longPressTriggered) {
      streamDeck.logger.debug("Long press was triggered, skipping select on keyUp");
      contextState.longPressTriggered = false;
      return;
    }

    const { settings, client } = contextState;

    if (!settings.serverAddress || !settings.cameraId) {
      streamDeck.logger.warn("Camera Select action not configured");
      await ev.action.showAlert();
      return;
    }

    if (!settings.gamepadSlot) {
      streamDeck.logger.warn("Gamepad slot not configured");
      await ev.action.showAlert();
      return;
    }

    if (!client || client.connectionStatus !== "connected") {
      streamDeck.logger.warn("Not connected to XTouch GW server");
      await ev.action.showAlert();
      return;
    }

    streamDeck.logger.info(`Short press - selecting camera ${settings.cameraId}`);

    try {
      await client.setCameraTarget(settings.gamepadSlot, settings.cameraId);
      streamDeck.logger.info(`Camera target set successfully: ${settings.gamepadSlot} -> ${settings.cameraId}`);
    } catch (error) {
      streamDeck.logger.error(`Failed to set camera target: ${error}`);
      await ev.action.showAlert();
    }
  }
}
