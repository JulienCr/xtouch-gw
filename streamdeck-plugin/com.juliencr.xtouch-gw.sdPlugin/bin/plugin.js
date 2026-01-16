import streamDeck, { action, SingletonAction, LogLevel } from '@elgato/streamdeck';
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
 * Check HTTP response and throw an error if not OK.
 * @param response The fetch Response object
 * @param operation Description of the operation for error messages
 */
async function checkResponse(response, operation) {
    if (!response.ok) {
        const errorText = await response.text();
        throw new Error(`Failed to ${operation}: HTTP ${response.status} - ${errorText}`);
    }
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
            // Map holds reference to object, so mutation is sufficient
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
     * @param slot The gamepad slot identifier
     * @param cameraId The camera ID to target
     */
    async setCameraTarget(slot, cameraId) {
        streamDeck.logger.info(`Setting camera target: slot=${slot}, camera=${cameraId}`);
        const url = `http://${this._serverAddress}/api/gamepad/${encodeURIComponent(slot)}/camera`;
        const response = await fetch(url, {
            method: "PUT",
            headers: {
                "Content-Type": "application/json",
            },
            body: JSON.stringify({
                camera_id: cameraId,
            }),
        });
        await checkResponse(response, "set camera target");
        streamDeck.logger.info(`Camera target set successfully: ${slot} -> ${cameraId}`);
    }
    /**
     * Fetch available gamepad slots via HTTP API
     */
    async getGamepadSlots() {
        const url = `http://${this._serverAddress}/api/gamepads`;
        const response = await fetch(url);
        await checkResponse(response, "fetch gamepad slots");
        return (await response.json());
    }
    /**
     * Fetch available cameras via HTTP API
     */
    async getCameras() {
        const url = `http://${this._serverAddress}/api/cameras`;
        const response = await fetch(url);
        await checkResponse(response, "fetch cameras");
        return (await response.json());
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
 * Scale a value proportionally to the button size.
 * @param baseValue The value at DEFAULT_BUTTON_SIZE
 * @param size Current button size
 * @returns Scaled and rounded value
 */
function scaled(baseValue, size) {
    return Math.round((size * baseValue) / DEFAULT_BUTTON_SIZE);
}
/**
 * Color constants for button rendering
 */
const Colors = {
    /** Background for inactive (not controlled) state */
    INACTIVE_BG: "#212121",
    /** Background for active (controlled by gamepad) state */
    ACTIVE_BG: "#1B5E20",
    /** Border color for ON AIR state */
    ON_AIR_BORDER: "#B71C1C",
    /** Indicator bar color for controlled state */
    CONTROLLED_INDICATOR: "#4CAF50",
    /** Text color */
    TEXT_COLOR: "#FFFFFF",
    /** Disconnected/error background */
    DISCONNECTED_BG: "#424242",
    /** Disconnected/error icon color */
    DISCONNECTED_ICON: "#FF5252",
};
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
 * Render a button image for a camera with the given state.
 *
 * Visual design:
 * - Inactive (not controlled): Dark gray (#212121) background
 * - Active (controlled by this gamepad): Dark green (#1B5E20) background + green bar at bottom
 * - On Air: Red 8px border (#B71C1C)
 * - Active + On Air: Dark green background + red border + green bar
 * - Camera icon in the center, text at the bottom
 *
 * @param state Button state including camera ID, controlled status, and on-air status
 * @param size Canvas size in pixels (default 144 for @2x resolution)
 * @returns Base64 data URL of the rendered PNG image
 */
function renderButtonImage(state, size = DEFAULT_BUTTON_SIZE) {
    const canvas = createCanvas(size, size);
    const ctx = canvas.getContext("2d");
    const borderWidth = scaled(10, size);
    const indicatorHeight = scaled(6, size);
    const fontSize = scaled(24, size);
    const padding = scaled(6, size);
    const iconSize = scaled(44, size);
    // Step 1: Draw background
    ctx.fillStyle = state.isControlled ? Colors.ACTIVE_BG : Colors.INACTIVE_BG;
    ctx.fillRect(0, 0, size, size);
    // Step 2: Draw ON AIR border (if isOnAir) with rounded corners matching Stream Deck buttons
    if (state.isOnAir) {
        ctx.strokeStyle = Colors.ON_AIR_BORDER;
        ctx.lineWidth = borderWidth;
        const offset = borderWidth / 2;
        const cornerRadius = Math.round(size * 7 / 72); // 6px at 72px button, scales to 12px at 144px
        ctx.beginPath();
        ctx.roundRect(offset, offset, size - borderWidth, size - borderWidth, cornerRadius);
        ctx.stroke();
    }
    // Step 3: Draw camera icon (centered, slightly above middle)
    const iconY = size * 0.38;
    drawCameraIcon(ctx, size / 2, iconY, iconSize, Colors.TEXT_COLOR);
    // Step 4: Draw camera name (at bottom)
    ctx.fillStyle = Colors.TEXT_COLOR;
    ctx.font = `bold ${fontSize}px sans-serif`;
    // Calculate available width for text (account for border and padding)
    const textPadding = state.isOnAir ? borderWidth + padding : padding;
    const availableWidth = size - textPadding * 2;
    // Position text near bottom, above the indicator if present
    const textY = state.isControlled
        ? size - indicatorHeight - padding - fontSize / 2 - (state.isOnAir ? borderWidth : 0)
        : size - padding - fontSize / 2 - (state.isOnAir ? borderWidth : 0);
    const displayText = truncateText(ctx, state.cameraId, availableWidth);
    drawCenteredText(ctx, displayText, size / 2, textY);
    // Step 5: Draw "controlled" indicator (green bar at bottom)
    if (state.isControlled) {
        ctx.fillStyle = Colors.CONTROLLED_INDICATOR;
        // Position indicator inside the border if ON AIR
        const indicatorX = state.isOnAir ? borderWidth : 0;
        const indicatorWidth = state.isOnAir ? size - borderWidth * 2 : size;
        const indicatorY = size - indicatorHeight - (state.isOnAir ? borderWidth : 0);
        ctx.fillRect(indicatorX, indicatorY, indicatorWidth, indicatorHeight);
    }
    // Return as data URL
    return canvas.toDataURL("image/png");
}
/**
 * Render a disconnected state button image.
 * Shows a dark gray background with a red "!" icon.
 *
 * @param size Canvas size in pixels (default 144 for @2x resolution)
 * @returns Base64 data URL of the rendered PNG image
 */
function renderDisconnectedImage(size = DEFAULT_BUTTON_SIZE) {
    const canvas = createCanvas(size, size);
    const ctx = canvas.getContext("2d");
    const fontSize = scaled(48, size);
    const labelFontSize = scaled(14, size);
    // Draw background
    ctx.fillStyle = Colors.DISCONNECTED_BG;
    ctx.fillRect(0, 0, size, size);
    // Draw exclamation mark icon
    ctx.fillStyle = Colors.DISCONNECTED_ICON;
    ctx.font = `bold ${fontSize}px sans-serif`;
    drawCenteredText(ctx, "!", size / 2, size / 2 - labelFontSize / 2);
    // Draw "Offline" label
    ctx.fillStyle = Colors.TEXT_COLOR;
    ctx.font = `${labelFontSize}px sans-serif`;
    drawCenteredText(ctx, "Offline", size / 2, size / 2 + fontSize / 2);
    return canvas.toDataURL("image/png");
}
/**
 * Render a "not configured" state button image.
 * Shows a dark gray background with a gear icon and "Config" label.
 *
 * @param size Canvas size in pixels (default 144 for @2x resolution)
 * @returns Base64 data URL of the rendered PNG image
 */
function renderNotConfiguredImage(size = DEFAULT_BUTTON_SIZE) {
    const canvas = createCanvas(size, size);
    const ctx = canvas.getContext("2d");
    const iconFontSize = scaled(36, size);
    const labelFontSize = scaled(14, size);
    // Draw background
    ctx.fillStyle = Colors.INACTIVE_BG;
    ctx.fillRect(0, 0, size, size);
    // Draw gear icon (using Unicode gear symbol)
    ctx.fillStyle = Colors.TEXT_COLOR;
    ctx.font = `${iconFontSize}px sans-serif`;
    drawCenteredText(ctx, "\u2699", size / 2, size / 2 - labelFontSize / 2);
    // Draw "Config" label
    ctx.font = `${labelFontSize}px sans-serif`;
    drawCenteredText(ctx, "Config", size / 2, size / 2 + iconFontSize / 2);
    return canvas.toDataURL("image/png");
}

/**
 * Normalize settings by providing empty string defaults for missing values.
 */
function normalizeSettings(settings) {
    return {
        serverAddress: settings.serverAddress || "",
        gamepadSlot: settings.gamepadSlot || "",
        cameraId: settings.cameraId || "",
    };
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
let CameraSelectAction = (() => {
    let _classDecorators = [action({ UUID: "com.juliencr.xtouch-gw.camera-select" })];
    let _classDescriptor;
    let _classExtraInitializers = [];
    let _classThis;
    let _classSuper = SingletonAction;
    _classThis = class extends _classSuper {
        constructor() {
            super(...arguments);
            /**
             * Map of context IDs to their state.
             * Each Stream Deck button instance has a unique context ID.
             */
            this.contexts = new Map();
        }
        /**
         * Called when the action appears on the Stream Deck.
         * Initializes the context state and connects to the server.
         */
        async onWillAppear(ev) {
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
            const contextState = {
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
        async onWillDisappear(ev) {
            const contextId = ev.action.id;
            streamDeck.logger.info(`Camera Select action disappeared: context=${contextId}`);
            // Remove context (client disconnection is handled separately via disconnectClient if needed)
            this.contexts.delete(contextId);
        }
        /**
         * Called when the key is pressed.
         * Sends the camera target request to the server.
         */
        async onKeyDown(ev) {
            const contextId = ev.action.id;
            const contextState = this.contexts.get(contextId);
            if (!contextState) {
                streamDeck.logger.warn(`No context state for ${contextId}`);
                await ev.action.showAlert();
                return;
            }
            const { settings, client } = contextState;
            streamDeck.logger.info(`Camera Select key pressed: context=${contextId}, camera=${settings.cameraId}, slot=${settings.gamepadSlot}`);
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
            }
            catch (error) {
                streamDeck.logger.error(`Failed to set camera target: ${error}`);
                await ev.action.showAlert();
            }
        }
        /**
         * Called when settings are received from the property inspector.
         * Updates stored settings and reconnects if necessary.
         */
        async onDidReceiveSettings(ev) {
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
        connectContext(contextId) {
            const contextState = this.contexts.get(contextId);
            if (!contextState)
                return;
            const { serverAddress } = contextState.settings;
            if (!serverAddress)
                return;
            streamDeck.logger.info(`Connecting context ${contextId} to ${serverAddress}`);
            // Get or create shared client
            const client = getClient(serverAddress);
            contextState.client = client;
            // Set up callbacks
            // Note: Multiple contexts may share the same client, so callbacks will update
            // all contexts that use this client when they're re-registered
            client.onStateChange = (state) => {
                this.handleStateChange(state);
            };
            client.onConnectionChange = (status) => {
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
        handleStateChange(state) {
            for (const [contextId, contextState] of this.contexts) {
                if (!contextState.client)
                    continue;
                const { gamepadSlot, cameraId } = contextState.settings;
                const wasActive = contextState.isActive;
                const wasOnAir = contextState.isOnAir;
                // Update active state
                if (gamepadSlot && cameraId) {
                    const gamepad = state.gamepads.get(gamepadSlot);
                    contextState.isActive = gamepad?.current_camera === cameraId;
                }
                else {
                    contextState.isActive = false;
                }
                // Update on-air state
                contextState.isOnAir = cameraId ? state.onAirCameraId === cameraId : false;
                // Only update display if state changed
                if (contextState.isActive !== wasActive || contextState.isOnAir !== wasOnAir) {
                    streamDeck.logger.debug(`State changed for ${contextId}: active=${contextState.isActive}, onAir=${contextState.isOnAir}`);
                    void this.updateDisplay(contextState);
                }
            }
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
         * Update context state from current client state.
         */
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
                client.getState();
                const { gamepadSlot, cameraId } = settings;
                if (gamepadSlot && cameraId) {
                    contextState.isActive = client.isControlledBy(cameraId, gamepadSlot);
                }
                else {
                    contextState.isActive = false;
                }
                contextState.isOnAir = cameraId ? client.isOnAir(cameraId) : false;
            }
            else {
                contextState.isActive = false;
                contextState.isOnAir = false;
            }
        }
        /**
         * Update the action display based on current state.
         * Renders button images showing camera name, active status, and connection state.
         */
        async updateDisplay(contextState) {
            const { settings, keyAction, isActive, isOnAir, connectionStatus } = contextState;
            try {
                let imageDataUrl;
                if (connectionStatus === "disconnected") {
                    // Show disconnected image with red "!" icon
                    imageDataUrl = renderDisconnectedImage();
                }
                else if (connectionStatus === "connecting") {
                    // Show connecting state - use text animation for now
                    await keyAction.setTitle("...");
                    return;
                }
                else if (!settings.cameraId) {
                    // Show not configured image with gear icon
                    imageDataUrl = renderNotConfiguredImage();
                }
                else {
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
            }
            catch (error) {
                // Fallback to title-based display if rendering fails
                streamDeck.logger.warn(`Failed to render button image, using title fallback: ${error}`);
                let title;
                if (connectionStatus === "disconnected") {
                    title = "!";
                }
                else if (!settings.cameraId) {
                    title = "Config";
                }
                else {
                    title = settings.cameraId;
                }
                try {
                    if (isActive && isOnAir) {
                        await keyAction.setTitle(`[LIVE]\n${title}`);
                    }
                    else if (isActive) {
                        await keyAction.setTitle(`[*]\n${title}`);
                    }
                    else if (isOnAir) {
                        await keyAction.setTitle(`(LIVE)\n${title}`);
                    }
                    else {
                        await keyAction.setTitle(title);
                    }
                }
                catch (fallbackError) {
                    // Action may have been removed, log but don't throw
                    streamDeck.logger.debug(`Failed to update display in fallback: ${fallbackError}`);
                }
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

// Configure logging
streamDeck.logger.setLevel(LogLevel.DEBUG);
// Register the camera select action
streamDeck.actions.registerAction(new CameraSelectAction());
// Connect to Stream Deck
streamDeck.connect();
streamDeck.logger.info("XTouch GW Camera Control plugin connected");
//# sourceMappingURL=plugin.js.map
