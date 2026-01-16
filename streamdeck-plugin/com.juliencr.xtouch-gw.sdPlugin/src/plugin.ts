import streamDeck, { LogLevel } from "@elgato/streamdeck";

import { CameraSelectAction } from "./actions/camera-select";

// Configure logging
streamDeck.logger.setLevel(LogLevel.DEBUG);

// Register the camera select action
streamDeck.actions.registerAction(new CameraSelectAction());

// Connect to Stream Deck
streamDeck.connect();

streamDeck.logger.info("XTouch GW Camera Control plugin connected");
