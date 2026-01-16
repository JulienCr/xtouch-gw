import { createCanvas, type Canvas, type CanvasRenderingContext2D } from "canvas";

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
} as const;

/**
 * Canvas context with computed scaled values for consistent rendering.
 */
interface RenderContext {
  canvas: Canvas;
  ctx: CanvasRenderingContext2D;
  size: number;
  scaled: (baseValue: number) => number;
}

/**
 * Create a render context with canvas and scaling helper.
 */
function createRenderContext(size: number = DEFAULT_BUTTON_SIZE): RenderContext {
  const canvas = createCanvas(size, size);
  const ctx = canvas.getContext("2d");
  return {
    canvas,
    ctx,
    size,
    scaled: (baseValue: number) => Math.round((size * baseValue) / DEFAULT_BUTTON_SIZE),
  };
}

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
 * - Inactive (not controlled): Dark gray background
 * - Active (controlled by this gamepad): Dark green background + green bar at bottom
 * - On Air: Red border
 * - Active + On Air: Dark green background + red border + green bar
 * - Camera icon in the center, text at the bottom
 */
export function renderButtonImage(state: ButtonState, size: number = DEFAULT_BUTTON_SIZE): string {
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
export function renderDisconnectedImage(size: number = DEFAULT_BUTTON_SIZE): string {
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
export function renderNotConfiguredImage(size: number = DEFAULT_BUTTON_SIZE): string {
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
export function renderResetButtonImage(state: ResetButtonState, size: number = DEFAULT_BUTTON_SIZE): string {
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
export function renderFlashImage(size: number = DEFAULT_BUTTON_SIZE): string {
  const { canvas, ctx } = createRenderContext(size);

  ctx.fillStyle = Colors.FLASH_BG;
  ctx.fillRect(0, 0, size, size);

  return canvas.toDataURL("image/png");
}
