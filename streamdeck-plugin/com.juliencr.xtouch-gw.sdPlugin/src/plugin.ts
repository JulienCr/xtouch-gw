import streamDeck, { LogLevel } from "@elgato/streamdeck";

import { CameraSelectAction } from "./actions/camera-select";
import { CameraResetAction } from "./actions/camera-reset";

// Configure logging
streamDeck.logger.setLevel(LogLevel.DEBUG);

// Register actions
streamDeck.actions.registerAction(new CameraSelectAction());
streamDeck.actions.registerAction(new CameraResetAction());

// Connect to Stream Deck
streamDeck.connect();

streamDeck.logger.info("XTouch GW Camera Control plugin connected");
