import streamDeck, { LogLevel } from "@elgato/streamdeck";

import { CameraSelectAction } from "./actions/camera-select";
import { CameraResetAction } from "./actions/camera-reset";
import { disconnectAllClients } from "./services/xtouch-client";

// Configure logging
streamDeck.logger.setLevel(LogLevel.INFO);

// Register actions
streamDeck.actions.registerAction(new CameraSelectAction());
streamDeck.actions.registerAction(new CameraResetAction());

// Cleanup WebSocket connections on process shutdown
process.on("SIGTERM", () => disconnectAllClients());
process.on("SIGINT", () => disconnectAllClients());

// Connect to Stream Deck
streamDeck.connect();

streamDeck.logger.info("XTouch GW Camera Control plugin connected");
