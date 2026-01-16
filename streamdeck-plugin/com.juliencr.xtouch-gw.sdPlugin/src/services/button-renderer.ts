import { createCanvas, type Canvas, type CanvasRenderingContext2D } from "canvas";

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
function scaled(baseValue: number, size: number): number {
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
  /** Flash/feedback background (yellow/amber) */
  FLASH_BG: "#F9A825",
} as const;

/**
 * Button state for rendering
 */
export interface ButtonState {
  /** Camera name to display */
  cameraId: string;
  /** Is this camera controlled by the gamepad? */
  isControlled: boolean;
  /** Is this camera currently on air (program)? */
  isOnAir: boolean;
}

/**
 * Reset button state for rendering
 */
export interface ResetButtonState {
  /** Camera name to display */
  cameraId: string;
}

/**
 * Truncate text to fit within a given width, adding "..." if needed.
 * @param ctx Canvas rendering context
 * @param text Text to truncate
 * @param maxWidth Maximum width in pixels
 * @returns Truncated text (with "..." if truncated)
 */
function truncateText(ctx: CanvasRenderingContext2D, text: string, maxWidth: number): string {
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
function drawCenteredText(ctx: CanvasRenderingContext2D, text: string, x: number, y: number): void {
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
function drawRoundedRect(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: number
): void {
  const rr = Math.max(0, Math.min(r, w / 2, h / 2));
  ctx.beginPath();
  // Fallback for contexts without roundRect
  if (typeof (ctx as any).roundRect === "function") {
    (ctx as any).roundRect(x, y, w, h, rr);
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
function drawCameraIcon(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  iconSize: number,
  color: string
): void {
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
function drawResetIcon(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  iconSize: number,
  color: string
): void {
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
export function renderButtonImage(state: ButtonState, size: number = DEFAULT_BUTTON_SIZE): string {
  const canvas: Canvas = createCanvas(size, size);
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
export function renderDisconnectedImage(size: number = DEFAULT_BUTTON_SIZE): string {
  const canvas: Canvas = createCanvas(size, size);
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
export function renderNotConfiguredImage(size: number = DEFAULT_BUTTON_SIZE): string {
  const canvas: Canvas = createCanvas(size, size);
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
 * Render a button image for a camera reset action.
 *
 * Visual design:
 * - Dark gray (#212121) background
 * - Reset icon (circular arrows) in center
 * - Camera ID text at bottom
 *
 * @param state Reset button state including camera ID
 * @param size Canvas size in pixels (default 144 for @2x resolution)
 * @returns Base64 data URL of the rendered PNG image
 */
export function renderResetButtonImage(state: ResetButtonState, size: number = DEFAULT_BUTTON_SIZE): string {
  const canvas: Canvas = createCanvas(size, size);
  const ctx = canvas.getContext("2d");

  const fontSize = scaled(24, size);
  const padding = scaled(6, size);
  const iconSize = scaled(44, size);

  // Step 1: Draw background (always inactive color - no state tracking for reset)
  ctx.fillStyle = Colors.INACTIVE_BG;
  ctx.fillRect(0, 0, size, size);

  // Step 2: Draw reset icon (centered, slightly above middle)
  const iconY = size * 0.38;
  drawResetIcon(ctx, size / 2, iconY, iconSize, Colors.TEXT_COLOR);

  // Step 3: Draw camera name (at bottom)
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
 *
 * @param size Canvas size in pixels (default 144 for @2x resolution)
 * @returns Base64 data URL of the rendered PNG image
 */
export function renderFlashImage(size: number = DEFAULT_BUTTON_SIZE): string {
  const canvas: Canvas = createCanvas(size, size);
  const ctx = canvas.getContext("2d");

  // Fill with yellow/amber color
  ctx.fillStyle = Colors.FLASH_BG;
  ctx.fillRect(0, 0, size, size);

  return canvas.toDataURL("image/png");
}
