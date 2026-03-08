import streamDeck from "@elgato/streamdeck";
import type { GamepadSlotInfo, CameraInfo } from "./xtouch-types";

/**
 * Make an HTTP request to the XTouch GW API.
 * Handles response checking and JSON parsing.
 */
export async function apiRequest<T>(
  baseUrl: string,
  path: string,
  options?: { method?: string; body?: object }
): Promise<T> {
  const url = `http://${baseUrl}${path}`;
  const fetchOptions: RequestInit = {
    method: options?.method ?? "GET",
    headers: { "Content-Type": "application/json" },
  };

  if (options?.body) {
    fetchOptions.body = JSON.stringify(options.body);
  }

  const response = await fetch(url, fetchOptions);

  if (!response.ok) {
    const errorText = await response.text();
    throw new Error(`API ${options?.method ?? "GET"} ${path}: HTTP ${response.status} - ${errorText}`);
  }

  const contentType = response.headers.get("content-type");
  if (contentType?.includes("application/json")) {
    return (await response.json()) as T;
  }

  return undefined as T;
}

/**
 * Set the camera target for a gamepad slot via HTTP API.
 *
 * @param serverAddress The server address (host:port)
 * @param slot The gamepad slot identifier
 * @param cameraId The camera ID to target
 * @param target Optional: "preview" or "program" to also switch OBS scene (default: "preview")
 */
export async function setCamera(
  serverAddress: string,
  slot: string,
  cameraId: string,
  target: "preview" | "program" = "preview"
): Promise<void> {
  streamDeck.logger.info(`Setting camera target: slot=${slot}, camera=${cameraId}, target=${target}`);
  const path = `/api/gamepad/${encodeURIComponent(slot)}/camera`;
  await apiRequest(serverAddress, path, { method: "PUT", body: { camera_id: cameraId, target } });
  streamDeck.logger.info(`Camera target set successfully: ${slot} -> ${cameraId} (${target})`);
}

/**
 * Reset a camera's zoom and/or position via HTTP API.
 *
 * @param serverAddress The server address (host:port)
 * @param cameraId The camera ID to reset
 * @param mode The reset mode: "position", "zoom", or "both"
 */
export async function resetCamera(
  serverAddress: string,
  cameraId: string,
  mode: "position" | "zoom" | "both"
): Promise<void> {
  streamDeck.logger.info(`Resetting camera: id=${cameraId}, mode=${mode}`);
  const path = `/api/cameras/${encodeURIComponent(cameraId)}/reset`;
  await apiRequest(serverAddress, path, { method: "POST", body: { mode } });
  streamDeck.logger.info(`Camera reset successful: ${cameraId}`);
}

/**
 * Fetch available gamepad slots via HTTP API.
 *
 * @param serverAddress The server address (host:port)
 * @returns Array of gamepad slot information
 */
export async function getGamepads(serverAddress: string): Promise<GamepadSlotInfo[]> {
  return apiRequest<GamepadSlotInfo[]>(serverAddress, "/api/gamepads");
}

/**
 * Fetch available cameras via HTTP API.
 *
 * @param serverAddress The server address (host:port)
 * @returns Array of camera information
 */
export async function getCameras(serverAddress: string): Promise<CameraInfo[]> {
  return apiRequest<CameraInfo[]>(serverAddress, "/api/cameras");
}
