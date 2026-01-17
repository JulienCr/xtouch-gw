import streamDeck, { SingletonAction, action, LogLevel } from '@elgato/streamdeck';
import { WebSocket } from 'ws';
import { createCanvas } from 'canvas';

/******************************************************************************
Copyright (c) Microsoft Corporation.

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY
AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM
LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR
OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR
PERFORMANCE OF THIS SOFTWARE.
***************************************************************************** */
/* global Reflect, Promise, SuppressedError, Symbol, Iterator */


function __esDecorate(ctor, descriptorIn, decorators, contextIn, initializers, extraInitializers) {
    function accept(f) { if (f !== void 0 && typeof f !== "function") throw new TypeError("Function expected"); return f; }
    var kind = contextIn.kind, key = kind === "getter" ? "get" : kind === "setter" ? "set" : "value";
    var target = !descriptorIn && ctor ? contextIn["static"] ? ctor : ctor.prototype : null;
    var descriptor = descriptorIn || (target ? Object.getOwnPropertyDescriptor(target, contextIn.name) : {});
    var _, done = false;
    for (var i = decorators.length - 1; i >= 0; i--) {
        var context = {};
        for (var p in contextIn) context[p] = p === "access" ? {} : contextIn[p];
        for (var p in contextIn.access) context.access[p] = contextIn.access[p];
        context.addInitializer = function (f) { if (done) throw new TypeError("Cannot add initializers after decoration has completed"); extraInitializers.push(accept(f || null)); };
        var result = (0, decorators[i])(kind === "accessor" ? { get: descriptor.get, set: descriptor.set } : descriptor[key], context);
        if (kind === "accessor") {
            if (result === void 0) continue;
            if (result === null || typeof result !== "object") throw new TypeError("Object expected");
            if (_ = accept(result.get)) descriptor.get = _;
            if (_ = accept(result.set)) descriptor.set = _;
            if (_ = accept(result.init)) initializers.unshift(_);
        }
        else if (_ = accept(result)) {
            if (kind === "field") initializers.unshift(_);
            else descriptor[key] = _;
        }
    }
    if (target) Object.defineProperty(target, contextIn.name, descriptor);
    done = true;
}
function __runInitializers(thisArg, initializers, value) {
    var useValue = arguments.length > 2;
    for (var i = 0; i < initializers.length; i++) {
        value = useValue ? initializers[i].call(thisArg, value) : initializers[i].call(thisArg);
    }
    return useValue ? value : void 0;
}
function __setFunctionName(f, name, prefix) {
    if (typeof name === "symbol") name = name.description ? "[".concat(name.description, "]") : "";
    return Object.defineProperty(f, "name", { configurable: true, value: prefix ? "".concat(prefix, " ", name) : name });
}
typeof SuppressedError === "function" ? SuppressedError : function (error, suppressed, message) {
    var e = new Error(message);
    return e.name = "SuppressedError", e.error = error, e.suppressed = suppressed, e;
};

/**
 * Make an HTTP request to the XTouch GW API.
 * Handles response checking and JSON parsing.
 */
async function apiRequest(baseUrl, path, options) {
    const url = `http://${baseUrl}${path}`;
    const fetchOptions = {
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
        return (await response.json());
    }
    return undefined;
}
/**
 * Client for communicating with the XTouch GW server.
 * Handles WebSocket connection for real-time state updates and HTTP API calls for camera targeting.
 */
class XTouchClient {
    constructor(serverAddress) {
        this._connectionStatus = "disconnected";
        this._ws = null;
        this._reconnectTimer = null;
        this._reconnectAttempts = 0;
        this._shouldReconnect = false;
        // State
        this._gamepads = new Map();
        this._cameras = new Map();
        this._onAirCameraId = null;
        // Callbacks
        this._onStateChange = null;
        this._onConnectionChange = null;
        this._serverAddress = serverAddress;
    }
    /**
     * Get the server address
     */
    get serverAddress() {
        return this._serverAddress;
    }
    /**
     * Get the current connection status
     */
    get connectionStatus() {
        return this._connectionStatus;
    }
    /**
     * Set callback for state changes
     */
    set onStateChange(callback) {
        this._onStateChange = callback;
    }
    /**
     * Set callback for connection status changes
     */
    set onConnectionChange(callback) {
        this._onConnectionChange = callback;
    }
    /**
     * Connect to the XTouch GW WebSocket server.
     * Automatically reconnects on disconnect with exponential backoff.
     */
    connect() {
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
    doConnect() {
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
        }
        catch (error) {
            streamDeck.logger.error(`Failed to create WebSocket: ${error}`);
            this.setConnectionStatus("disconnected");
            this.scheduleReconnect();
        }
    }
    /**
     * Set up WebSocket event handlers
     */
    setupWebSocketHandlers() {
        if (!this._ws)
            return;
        this._ws.on("open", () => {
            streamDeck.logger.info("WebSocket connected to XTouch GW");
            this._reconnectAttempts = 0;
            this.setConnectionStatus("connected");
        });
        this._ws.on("close", (code, reason) => {
            streamDeck.logger.info(`WebSocket closed: code=${code}, reason=${reason.toString()}`);
            this._ws = null;
            this.setConnectionStatus("disconnected");
            if (this._shouldReconnect) {
                this.scheduleReconnect();
            }
        });
        this._ws.on("error", (error) => {
            streamDeck.logger.error(`WebSocket error: ${error.message}`);
            // onclose will be called after onerror, so we handle reconnection there
        });
        this._ws.on("message", (data) => {
            this.handleMessage(data.toString());
        });
    }
    /**
     * Handle incoming WebSocket message
     */
    handleMessage(data) {
        try {
            const message = JSON.parse(data);
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
                    streamDeck.logger.warn(`Unknown message type: ${message.type}`);
            }
        }
        catch (error) {
            streamDeck.logger.error(`Failed to parse WebSocket message: ${error}`);
        }
    }
    /**
     * Handle snapshot message (full state on connect)
     */
    handleSnapshot(message) {
        streamDeck.logger.info(`Received snapshot: ${message.gamepads.length} gamepads, ${message.cameras.length} cameras`);
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
    handleTargetChanged(message) {
        streamDeck.logger.info(`Camera target changed: ${message.gamepad_slot} -> ${message.camera_id}`);
        const gamepad = this._gamepads.get(message.gamepad_slot);
        if (gamepad) {
            // Object retrieved from Map is mutated directly; no need to re-set since Map still references the same instance
            gamepad.current_camera = message.camera_id;
        }
        else {
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
    handleOnAirChanged(message) {
        streamDeck.logger.info(`On-air camera changed: ${message.camera_id} (scene: ${message.scene_name})`);
        this._onAirCameraId = message.camera_id;
        this.emitStateChange();
    }
    /**
     * Schedule a reconnection attempt with exponential backoff
     */
    scheduleReconnect() {
        if (!this._shouldReconnect)
            return;
        const delay = Math.min(XTouchClient.INITIAL_RECONNECT_DELAY_MS * Math.pow(2, this._reconnectAttempts), XTouchClient.MAX_RECONNECT_DELAY_MS);
        this._reconnectAttempts++;
        streamDeck.logger.info(`Scheduling reconnect attempt ${this._reconnectAttempts} in ${delay}ms`);
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
    disconnect() {
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
     * Set connection status and emit change event
     */
    setConnectionStatus(status) {
        if (this._connectionStatus === status)
            return;
        this._connectionStatus = status;
        if (this._onConnectionChange) {
            try {
                this._onConnectionChange(status);
            }
            catch (error) {
                streamDeck.logger.error(`Error in connection change callback: ${error}`);
            }
        }
    }
    /**
     * Emit state change event
     */
    emitStateChange() {
        if (this._onStateChange) {
            try {
                this._onStateChange(this.getState());
            }
            catch (error) {
                streamDeck.logger.error(`Error in state change callback: ${error}`);
            }
        }
    }
    /**
     * Get current state snapshot
     */
    getState() {
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
    isControlledBy(cameraId, gamepadSlot) {
        const gamepad = this._gamepads.get(gamepadSlot);
        return gamepad?.current_camera === cameraId;
    }
    /**
     * Check if a camera is currently on air
     */
    isOnAir(cameraId) {
        return this._onAirCameraId === cameraId;
    }
    /**
     * Set the camera target for a gamepad slot via HTTP API.
     *
     * @param slot The gamepad slot identifier
     * @param cameraId The camera ID to target
     * @param target Optional: "preview" or "program" to also switch OBS scene (default: "preview")
     */
    async setCameraTarget(slot, cameraId, target = "preview") {
        streamDeck.logger.info(`Setting camera target: slot=${slot}, camera=${cameraId}, target=${target}`);
        const path = `/api/gamepad/${encodeURIComponent(slot)}/camera`;
        await apiRequest(this._serverAddress, path, { method: "PUT", body: { camera_id: cameraId, target } });
        streamDeck.logger.info(`Camera target set successfully: ${slot} -> ${cameraId} (${target})`);
    }
    /**
     * Fetch available gamepad slots via HTTP API.
     */
    async getGamepadSlots() {
        return apiRequest(this._serverAddress, "/api/gamepads");
    }
    /**
     * Fetch available cameras via HTTP API.
     */
    async getCameras() {
        return apiRequest(this._serverAddress, "/api/cameras");
    }
    /**
     * Reset a camera's zoom and/or position via HTTP API.
     */
    async resetCamera(cameraId, mode) {
        streamDeck.logger.info(`Resetting camera: id=${cameraId}, mode=${mode}`);
        const path = `/api/cameras/${encodeURIComponent(cameraId)}/reset`;
        await apiRequest(this._serverAddress, path, { method: "POST", body: { mode } });
        streamDeck.logger.info(`Camera reset successful: ${cameraId}`);
    }
}
// Reconnect configuration
XTouchClient.INITIAL_RECONNECT_DELAY_MS = 1000;
XTouchClient.MAX_RECONNECT_DELAY_MS = 30000;
// WebSocket close codes
XTouchClient.CLOSE_NORMAL = 1000;
/**
 * Client instances per server address (singleton pattern)
 */
const clientInstances = new Map();
/**
 * Get or create a client instance for a server address.
 * Multiple actions can share the same client instance per server.
 *
 * @param serverAddress The server address (host:port)
 * @returns The XTouchClient instance for that server
 */
function getClient(serverAddress) {
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
 * Default button size in pixels (Stream Deck @2x resolution)
 */
const DEFAULT_BUTTON_SIZE = 144;
/**
 * Color constants for button rendering
 */
const Colors = {
    INACTIVE_BG: "#212121",
    ACTIVE_BG: "#1B5E20",
    ON_AIR_BORDER: "#B71C1C",
    CONTROLLED_INDICATOR: "#4CAF50",
    TEXT_COLOR: "#FFFFFF",
    DISCONNECTED_BG: "#424242",
    DISCONNECTED_ICON: "#FF5252",
    FLASH_BG: "#F9A825",
};
/**
 * Create a render context with canvas and scaling helper.
 */
function createRenderContext(size = DEFAULT_BUTTON_SIZE) {
    const canvas = createCanvas(size, size);
    const ctx = canvas.getContext("2d");
    return {
        canvas,
        ctx,
        size,
        scaled: (baseValue) => Math.round((size * baseValue) / DEFAULT_BUTTON_SIZE),
    };
}
/**
 * Truncate text to fit within a given width, adding "..." if needed.
 * @param ctx Canvas rendering context
 * @param text Text to truncate
 * @param maxWidth Maximum width in pixels
 * @returns Truncated text (with "..." if truncated)
 */
function truncateText(ctx, text, maxWidth) {
    const metrics = ctx.measureText(text);
    if (metrics.width <= maxWidth) {
        return text;
    }
    const ellipsis = "...";
    let truncated = text;
    while (truncated.length > 0) {
        truncated = truncated.slice(0, -1);
        const testText = truncated + ellipsis;
        const testMetrics = ctx.measureText(testText);
        if (testMetrics.width <= maxWidth) {
            return testText;
        }
    }
    return ellipsis;
}
/**
 * Draw centered text on the canvas.
 * @param ctx Canvas rendering context
 * @param text Text to draw
 * @param x Center X coordinate
 * @param y Center Y coordinate
 */
function drawCenteredText(ctx, text, x, y) {
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.fillText(text, x, y);
}
/**
 * Draw a rounded rectangle path.
 * Falls back to manual path construction if roundRect is not available.
 * @param ctx Canvas rendering context
 * @param x Top-left X coordinate
 * @param y Top-left Y coordinate
 * @param w Width
 * @param h Height
 * @param r Corner radius
 */
function drawRoundedRect(ctx, x, y, w, h, r) {
    const rr = Math.max(0, Math.min(r, w / 2, h / 2));
    ctx.beginPath();
    // Fallback for contexts without roundRect
    if (typeof ctx.roundRect === "function") {
        ctx.roundRect(x, y, w, h, rr);
        return;
    }
    ctx.moveTo(x + rr, y);
    ctx.lineTo(x + w - rr, y);
    ctx.quadraticCurveTo(x + w, y, x + w, y + rr);
    ctx.lineTo(x + w, y + h - rr);
    ctx.quadraticCurveTo(x + w, y + h, x + w - rr, y + h);
    ctx.lineTo(x + rr, y + h);
    ctx.quadraticCurveTo(x, y + h, x, y + h - rr);
    ctx.lineTo(x, y + rr);
    ctx.quadraticCurveTo(x, y, x + rr, y);
}
/**
 * Draw a video camera icon using canvas paths
 * @param ctx Canvas rendering context
 * @param x Center X coordinate
 * @param y Center Y coordinate
 * @param iconSize Size of the icon
 * @param color Fill color
 */
function drawCameraIcon(ctx, x, y, iconSize, color) {
    const lineWidth = Math.max(2, iconSize / 12);
    ctx.save();
    ctx.fillStyle = color;
    ctx.strokeStyle = color;
    ctx.lineWidth = lineWidth;
    ctx.lineCap = "round";
    ctx.lineJoin = "round";
    // Body (centered)
    const bodyWidth = iconSize * 0.62;
    const bodyHeight = iconSize * 0.44;
    const bodyX = x - (bodyWidth / 2 + iconSize * 0.08);
    const bodyY = y - bodyHeight / 2;
    const cornerRadius = iconSize * 0.10;
    drawRoundedRect(ctx, bodyX, bodyY, bodyWidth, bodyHeight, cornerRadius);
    ctx.fill();
    // Lens (highlight + pupil)
    const lensRadius = iconSize * 0.12;
    const lensX = bodyX + iconSize * 0.18;
    const lensY = y;
    ctx.save();
    ctx.globalAlpha = 0.28;
    ctx.beginPath();
    ctx.arc(lensX, lensY, lensRadius, 0, Math.PI * 2);
    ctx.fill();
    ctx.restore();
    ctx.beginPath();
    ctx.arc(lensX, lensY, lensRadius * 0.45, 0, Math.PI * 2);
    ctx.fill();
    // Right viewfinder block
    const vfW = iconSize * 0.22;
    const vfH = iconSize * 0.26;
    const vfX = bodyX + bodyWidth;
    const vfY = y - vfH / 2;
    const vfR = iconSize * 0.06;
    drawRoundedRect(ctx, vfX, vfY, vfW, vfH, vfR);
    ctx.fill();
    ctx.restore();
}
/**
 * Draw a reset icon (two circular arrows forming a refresh/reset symbol)
 * @param ctx Canvas rendering context
 * @param x Center X coordinate
 * @param y Center Y coordinate
 * @param iconSize Size of the icon
 * @param color Stroke color
 */
function drawResetIcon(ctx, x, y, iconSize, color) {
    const lineWidth = Math.max(2, iconSize / 10);
    const radius = iconSize * 0.35;
    const arrowSize = iconSize * 0.15;
    ctx.save();
    ctx.strokeStyle = color;
    ctx.fillStyle = color;
    ctx.lineWidth = lineWidth;
    ctx.lineCap = "round";
    ctx.lineJoin = "round";
    // Draw two arc arrows forming a circular reset symbol
    // First arc (top-right, going clockwise)
    ctx.beginPath();
    ctx.arc(x, y, radius, -Math.PI * 0.15, Math.PI * 0.7, false);
    ctx.stroke();
    // Arrow head for first arc (pointing down-left)
    const angle1 = Math.PI * 0.7;
    const ax1 = x + radius * Math.cos(angle1);
    const ay1 = y + radius * Math.sin(angle1);
    ctx.beginPath();
    ctx.moveTo(ax1, ay1);
    ctx.lineTo(ax1 - arrowSize * 0.8, ay1 - arrowSize * 0.5);
    ctx.lineTo(ax1 + arrowSize * 0.3, ay1 - arrowSize * 0.8);
    ctx.closePath();
    ctx.fill();
    // Second arc (bottom-left, going clockwise)
    ctx.beginPath();
    ctx.arc(x, y, radius, Math.PI * 0.85, Math.PI * 1.7, false);
    ctx.stroke();
    // Arrow head for second arc (pointing up-right)
    const angle2 = Math.PI * 1.7;
    const ax2 = x + radius * Math.cos(angle2);
    const ay2 = y + radius * Math.sin(angle2);
    ctx.beginPath();
    ctx.moveTo(ax2, ay2);
    ctx.lineTo(ax2 + arrowSize * 0.8, ay2 + arrowSize * 0.5);
    ctx.lineTo(ax2 - arrowSize * 0.3, ay2 + arrowSize * 0.8);
    ctx.closePath();
    ctx.fill();
    ctx.restore();
}
/**
 * Render a button image for a camera with the given state.
 *
 * Visual design:
 * - Inactive (not controlled): Dark gray background
 * - Active (controlled by this gamepad): Dark green background + green bar at bottom
 * - On Air: Red border
 * - Active + On Air: Dark green background + red border + green bar
 * - Camera icon in the center, text at the bottom
 */
function renderButtonImage(state, size = DEFAULT_BUTTON_SIZE) {
    const { canvas, ctx, scaled } = createRenderContext(size);
    const borderWidth = scaled(10);
    const indicatorHeight = scaled(6);
    const fontSize = scaled(24);
    const padding = scaled(6);
    const iconSize = scaled(44);
    // Background
    ctx.fillStyle = state.isControlled ? Colors.ACTIVE_BG : Colors.INACTIVE_BG;
    ctx.fillRect(0, 0, size, size);
    // ON AIR border
    if (state.isOnAir) {
        ctx.strokeStyle = Colors.ON_AIR_BORDER;
        ctx.lineWidth = borderWidth;
        const offset = borderWidth / 2;
        const cornerRadius = Math.round(size * 7 / 72);
        ctx.beginPath();
        ctx.roundRect(offset, offset, size - borderWidth, size - borderWidth, cornerRadius);
        ctx.stroke();
    }
    // Camera icon
    const iconY = size * 0.38;
    drawCameraIcon(ctx, size / 2, iconY, iconSize, Colors.TEXT_COLOR);
    // Camera name text
    ctx.fillStyle = Colors.TEXT_COLOR;
    ctx.font = `bold ${fontSize}px sans-serif`;
    const textPadding = state.isOnAir ? borderWidth + padding : padding;
    const availableWidth = size - textPadding * 2;
    const borderOffset = state.isOnAir ? borderWidth : 0;
    const textY = state.isControlled
        ? size - indicatorHeight - padding - fontSize / 2 - borderOffset
        : size - padding - fontSize / 2 - borderOffset;
    const displayText = truncateText(ctx, state.cameraId, availableWidth);
    drawCenteredText(ctx, displayText, size / 2, textY);
    // Controlled indicator bar
    if (state.isControlled) {
        ctx.fillStyle = Colors.CONTROLLED_INDICATOR;
        const indicatorX = state.isOnAir ? borderWidth : 0;
        const indicatorWidth = state.isOnAir ? size - borderWidth * 2 : size;
        const indicatorY = size - indicatorHeight - borderOffset;
        ctx.fillRect(indicatorX, indicatorY, indicatorWidth, indicatorHeight);
    }
    return canvas.toDataURL("image/png");
}
/**
 * Render a disconnected state button image.
 * Shows a dark gray background with a red "!" icon.
 */
function renderDisconnectedImage(size = DEFAULT_BUTTON_SIZE) {
    const { canvas, ctx, scaled } = createRenderContext(size);
    const fontSize = scaled(48);
    const labelFontSize = scaled(14);
    ctx.fillStyle = Colors.DISCONNECTED_BG;
    ctx.fillRect(0, 0, size, size);
    ctx.fillStyle = Colors.DISCONNECTED_ICON;
    ctx.font = `bold ${fontSize}px sans-serif`;
    drawCenteredText(ctx, "!", size / 2, size / 2 - labelFontSize / 2);
    ctx.fillStyle = Colors.TEXT_COLOR;
    ctx.font = `${labelFontSize}px sans-serif`;
    drawCenteredText(ctx, "Offline", size / 2, size / 2 + fontSize / 2);
    return canvas.toDataURL("image/png");
}
/**
 * Render a "not configured" state button image.
 * Shows a dark gray background with a gear icon and "Config" label.
 */
function renderNotConfiguredImage(size = DEFAULT_BUTTON_SIZE) {
    const { canvas, ctx, scaled } = createRenderContext(size);
    const iconFontSize = scaled(36);
    const labelFontSize = scaled(14);
    ctx.fillStyle = Colors.INACTIVE_BG;
    ctx.fillRect(0, 0, size, size);
    ctx.fillStyle = Colors.TEXT_COLOR;
    ctx.font = `${iconFontSize}px sans-serif`;
    drawCenteredText(ctx, "\u2699", size / 2, size / 2 - labelFontSize / 2);
    ctx.font = `${labelFontSize}px sans-serif`;
    drawCenteredText(ctx, "Config", size / 2, size / 2 + iconFontSize / 2);
    return canvas.toDataURL("image/png");
}
/**
 * Render a button image for a camera reset action.
 *
 * Visual design:
 * - Dark gray background
 * - Reset icon (circular arrows) in center
 * - Camera ID text at bottom
 */
function renderResetButtonImage(state, size = DEFAULT_BUTTON_SIZE) {
    const { canvas, ctx, scaled } = createRenderContext(size);
    const fontSize = scaled(24);
    const padding = scaled(6);
    const iconSize = scaled(44);
    ctx.fillStyle = Colors.INACTIVE_BG;
    ctx.fillRect(0, 0, size, size);
    const iconY = size * 0.38;
    drawResetIcon(ctx, size / 2, iconY, iconSize, Colors.TEXT_COLOR);
    ctx.fillStyle = Colors.TEXT_COLOR;
    ctx.font = `bold ${fontSize}px sans-serif`;
    const availableWidth = size - padding * 2;
    const textY = size - padding - fontSize / 2;
    const displayText = truncateText(ctx, state.cameraId, availableWidth);
    drawCenteredText(ctx, displayText, size / 2, textY);
    return canvas.toDataURL("image/png");
}
/**
 * Render a yellow flash image for feedback.
 * Used for reset confirmation instead of the green checkmark.
 */
function renderFlashImage(size = DEFAULT_BUTTON_SIZE) {
    const { canvas, ctx } = createRenderContext(size);
    ctx.fillStyle = Colors.FLASH_BG;
    ctx.fillRect(0, 0, size, size);
    return canvas.toDataURL("image/png");
}

/**
 * Configuration for the blink animation on key press.
 */
const BLINK_CONFIG = {
    ITERATIONS: 2,
    DURATION_MS: 100,
};
/**
 * Execute a yellow blink animation on a key action.
 * Used for visual feedback after successful operations.
 *
 * @param keyAction The Stream Deck key action
 * @param restoreDisplay Callback to restore the normal display after blinking
 */
async function executeBlinkAnimation(keyAction, restoreDisplay) {
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
class CameraActionBase extends SingletonAction {
    constructor() {
        super(...arguments);
        /**
         * Map of context IDs to their state.
         * Each Stream Deck button instance has a unique context ID.
         */
        this.contexts = new Map();
    }
    /**
     * Update action-specific state from the current client.
     * Override in subclasses that track additional state beyond connection status.
     * Base implementation only updates connection status.
     */
    updateStateFromClient(contextState) {
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
    async onWillAppear(ev) {
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
    async onWillDisappear(ev) {
        const contextId = ev.action.id;
        streamDeck.logger.info(`Action disappeared: context=${contextId}`);
        this.contexts.delete(contextId);
    }
    /**
     * Called when settings are received from the property inspector.
     * Updates stored settings and reconnects if necessary.
     */
    async onDidReceiveSettings(ev) {
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
    disconnectContextClient(contextState) {
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
    connectContext(contextId) {
        const contextState = this.contexts.get(contextId);
        if (!contextState)
            return;
        const { serverAddress } = contextState.settings;
        if (!serverAddress)
            return;
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
    handleConnectionChange(status, serverAddress) {
        const normalizedAddress = serverAddress.toLowerCase().trim();
        for (const [contextId, contextState] of this.contexts) {
            if (contextState.settings.serverAddress.toLowerCase().trim() !== normalizedAddress) {
                continue;
            }
            if (contextState.connectionStatus !== status) {
                streamDeck.logger.info(`Connection status changed for ${contextId}: ${contextState.connectionStatus} -> ${status}`);
                contextState.connectionStatus = status;
                void this.updateDisplay(contextState);
            }
        }
    }
    /**
     * Update the action display based on current state.
     * Renders button images showing camera name and connection state.
     */
    async updateDisplay(contextState) {
        const { keyAction, connectionStatus } = contextState;
        if (connectionStatus === "connecting") {
            await keyAction.setTitle("...");
            return;
        }
        try {
            const imageDataUrl = this.getDisplayImage(contextState);
            await keyAction.setTitle("");
            await keyAction.setImage(imageDataUrl);
        }
        catch (error) {
            streamDeck.logger.warn(`Failed to render button image, using title fallback: ${error}`);
            const title = this.getFallbackTitle(contextState);
            try {
                await keyAction.setTitle(title);
            }
            catch (fallbackError) {
                streamDeck.logger.debug(`Failed to update display in fallback: ${fallbackError}`);
            }
        }
    }
    /**
     * Get the appropriate display image based on connection status and configuration.
     */
    getDisplayImage(contextState) {
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
    async executeCameraReset(contextState, resetMode, keyAction) {
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
        }
        catch (error) {
            streamDeck.logger.error(`Failed to reset camera: ${error}`);
            await keyAction.showAlert();
            return false;
        }
    }
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
let CameraSelectAction = (() => {
    let _classDecorators = [action({ UUID: "com.juliencr.xtouch-gw.camera-select" })];
    let _classDescriptor;
    let _classExtraInitializers = [];
    let _classThis;
    let _classSuper = CameraActionBase;
    _classThis = class extends _classSuper {
        normalizeSettings(settings) {
            return {
                serverAddress: settings.serverAddress || "",
                gamepadSlot: settings.gamepadSlot || "",
                cameraId: settings.cameraId || "",
                resetMode: settings.resetMode || "both",
            };
        }
        createContextState(settings, keyAction) {
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
        renderImage(contextState) {
            return renderButtonImage({
                cameraId: contextState.settings.cameraId,
                isControlled: contextState.isActive,
                isOnAir: contextState.isOnAir,
            });
        }
        getFallbackTitle(contextState) {
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
        setupClientCallbacks(client, serverAddress) {
            client.onStateChange = (state) => {
                this.handleStateChange(state);
            };
            client.onConnectionChange = (status) => {
                this.handleConnectionChange(status, serverAddress);
            };
        }
        updateStateFromClient(contextState) {
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
            }
            else {
                contextState.isActive = false;
                contextState.isOnAir = false;
            }
        }
        /**
         * Called when the action disappears from the Stream Deck.
         * Clears the long press timer to prevent orphaned callbacks.
         */
        async onWillDisappear(ev) {
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
        handleStateChange(state) {
            for (const [contextId, contextState] of this.contexts) {
                if (!contextState.client)
                    continue;
                const { gamepadSlot, cameraId } = contextState.settings;
                const wasActive = contextState.isActive;
                const wasOnAir = contextState.isOnAir;
                if (gamepadSlot && cameraId) {
                    const gamepad = state.gamepads.get(gamepadSlot);
                    contextState.isActive = gamepad?.current_camera === cameraId;
                }
                else {
                    contextState.isActive = false;
                }
                contextState.isOnAir = cameraId ? state.onAirCameraId === cameraId : false;
                if (contextState.isActive !== wasActive || contextState.isOnAir !== wasOnAir) {
                    streamDeck.logger.debug(`State changed for ${contextId}: active=${contextState.isActive}, onAir=${contextState.isOnAir}`);
                    void this.updateDisplay(contextState);
                }
            }
        }
        /**
         * Called when the key is pressed.
         * Starts a timer for long press detection - reset triggers automatically after 500ms.
         */
        async onKeyDown(ev) {
            const contextId = ev.action.id;
            const contextState = this.contexts.get(contextId);
            if (!contextState)
                return;
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
        async executeReset(contextId, keyAction) {
            const contextState = this.contexts.get(contextId);
            if (!contextState)
                return;
            streamDeck.logger.info(`Long press triggered - resetting camera ${contextState.settings.cameraId}`);
            await this.executeCameraReset(contextState, contextState.settings.resetMode || "both", keyAction);
        }
        /**
         * Called when the key is released.
         * If released before 500ms, cancels reset timer and executes camera select.
         * If reset already triggered, does nothing.
         */
        async onKeyUp(ev) {
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
            }
            catch (error) {
                streamDeck.logger.error(`Failed to set camera target: ${error}`);
                await ev.action.showAlert();
            }
        }
    };
    __setFunctionName(_classThis, "CameraSelectAction");
    (() => {
        const _metadata = typeof Symbol === "function" && Symbol.metadata ? Object.create(_classSuper[Symbol.metadata] ?? null) : void 0;
        __esDecorate(null, _classDescriptor = { value: _classThis }, _classDecorators, { kind: "class", name: _classThis.name, metadata: _metadata }, null, _classExtraInitializers);
        _classThis = _classDescriptor.value;
        if (_metadata) Object.defineProperty(_classThis, Symbol.metadata, { enumerable: true, configurable: true, writable: true, value: _metadata });
        __runInitializers(_classThis, _classExtraInitializers);
    })();
    return _classThis;
})();

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
let CameraResetAction = (() => {
    let _classDecorators = [action({ UUID: "com.juliencr.xtouch-gw.camera-reset" })];
    let _classDescriptor;
    let _classExtraInitializers = [];
    let _classThis;
    let _classSuper = CameraActionBase;
    _classThis = class extends _classSuper {
        normalizeSettings(settings) {
            return {
                serverAddress: settings.serverAddress || "",
                cameraId: settings.cameraId || "",
                resetMode: settings.resetMode || "both",
            };
        }
        createContextState(settings, keyAction) {
            return {
                settings,
                client: null,
                keyAction,
                connectionStatus: "disconnected",
            };
        }
        renderImage(contextState) {
            return renderResetButtonImage({ cameraId: contextState.settings.cameraId });
        }
        getFallbackTitle(contextState) {
            const { connectionStatus, settings } = contextState;
            if (connectionStatus === "disconnected") {
                return "!";
            }
            if (!settings.cameraId) {
                return "Config";
            }
            return settings.cameraId;
        }
        setupClientCallbacks(client, serverAddress) {
            client.onConnectionChange = (status) => {
                this.handleConnectionChange(status, serverAddress);
            };
        }
        /**
         * Called when the key is pressed.
         * Sends the camera reset request to the server.
         */
        async onKeyDown(ev) {
            const contextId = ev.action.id;
            const contextState = this.contexts.get(contextId);
            if (!contextState) {
                streamDeck.logger.warn(`No context state for ${contextId}`);
                await ev.action.showAlert();
                return;
            }
            streamDeck.logger.info(`Camera Reset key pressed: context=${contextId}, camera=${contextState.settings.cameraId}, mode=${contextState.settings.resetMode}`);
            await this.executeCameraReset(contextState, contextState.settings.resetMode, ev.action);
        }
    };
    __setFunctionName(_classThis, "CameraResetAction");
    (() => {
        const _metadata = typeof Symbol === "function" && Symbol.metadata ? Object.create(_classSuper[Symbol.metadata] ?? null) : void 0;
        __esDecorate(null, _classDescriptor = { value: _classThis }, _classDecorators, { kind: "class", name: _classThis.name, metadata: _metadata }, null, _classExtraInitializers);
        _classThis = _classDescriptor.value;
        if (_metadata) Object.defineProperty(_classThis, Symbol.metadata, { enumerable: true, configurable: true, writable: true, value: _metadata });
        __runInitializers(_classThis, _classExtraInitializers);
    })();
    return _classThis;
})();

// Configure logging
streamDeck.logger.setLevel(LogLevel.DEBUG);
// Register actions
streamDeck.actions.registerAction(new CameraSelectAction());
streamDeck.actions.registerAction(new CameraResetAction());
// Connect to Stream Deck
streamDeck.connect();
streamDeck.logger.info("XTouch GW Camera Control plugin connected");
//# sourceMappingURL=plugin.js.map
