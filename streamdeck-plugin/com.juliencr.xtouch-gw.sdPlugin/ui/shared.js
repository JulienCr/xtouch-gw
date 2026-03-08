/**
 * Shared Property Inspector logic for XTouch GW Stream Deck plugin.
 *
 * Each action-specific HTML must define the following before this script loads:
 *
 *   window.piConfig = {
 *     defaultSettings: { serverAddress: "localhost:8125", ... },
 *     applySettingsToUI: function(settings) { ... },
 *     buildSettingsFromPayload: function(payload) { ... },
 *     setupActionListeners: function(saveSettingsFn) { ... },
 *     fetchDropdownData: async function(serverAddress) { ... },
 *     resetDropdowns: function() { ... }
 *   };
 */

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------
var DEFAULT_SERVER_ADDRESS = "localhost:8125";

// ---------------------------------------------------------------------------
// Stream Deck WebSocket connection state
// ---------------------------------------------------------------------------
var websocket = null;
var uuid = null;
var actionInfo = null;
var sdInfo = null;

// ---------------------------------------------------------------------------
// Shared UI state
// ---------------------------------------------------------------------------
var currentSettings = {};
var debounceTimer = null;
var isConnected = false;
var listenersInitialized = false;

// ---------------------------------------------------------------------------
// Shared DOM elements (expected in every PI page)
// ---------------------------------------------------------------------------
var serverAddressInput = null;
var statusDot = null;
var statusText = null;
var testButton = null;

// ---------------------------------------------------------------------------
// Dropdown helpers (used by action-specific code)
// ---------------------------------------------------------------------------
function setDropdownLoading(select, message) {
  select.innerHTML = '<option value="" class="loading-option">' + message + '</option>';
  select.disabled = true;
}

function setDropdownError(select, message) {
  select.innerHTML = '<option value="" class="error-option">' + message + '</option>';
  select.disabled = true;
}

function populateCameraDropdown(cameraIdSelect, cameras) {
  cameraIdSelect.innerHTML = '<option value="">-- Select camera --</option>';

  if (Array.isArray(cameras)) {
    cameras.forEach(function(camera) {
      var option = document.createElement("option");
      if (typeof camera === "object") {
        option.value = camera.id || camera.camera_id || "";
        option.textContent = camera.name || camera.id || camera.camera_id || "Unknown";
      } else {
        option.value = camera;
        option.textContent = camera;
      }
      cameraIdSelect.appendChild(option);
    });
    cameraIdSelect.disabled = false;
  } else if (typeof cameras === "object" && cameras !== null) {
    Object.keys(cameras).forEach(function(cameraId) {
      var option = document.createElement("option");
      option.value = cameraId;
      var info = cameras[cameraId];
      option.textContent = (info && info.name) ? info.name : cameraId;
      cameraIdSelect.appendChild(option);
    });
    cameraIdSelect.disabled = false;
  }
}

function populateGamepadDropdown(gamepadSlotSelect, gamepads) {
  gamepadSlotSelect.innerHTML = '<option value="">-- Select gamepad --</option>';

  if (Array.isArray(gamepads)) {
    gamepads.forEach(function(gamepad) {
      var option = document.createElement("option");
      if (typeof gamepad === "object") {
        option.value = gamepad.slot || gamepad.id || "";
        var cameraInfo = gamepad.camera_id ? " [" + gamepad.camera_id + "]" : "";
        option.textContent = (gamepad.name || gamepad.slot || gamepad.id || "Unknown") + cameraInfo;
      } else {
        option.value = gamepad;
        option.textContent = gamepad;
      }
      gamepadSlotSelect.appendChild(option);
    });
    gamepadSlotSelect.disabled = false;
  } else if (typeof gamepads === "object" && gamepads !== null) {
    Object.keys(gamepads).forEach(function(slot) {
      var option = document.createElement("option");
      option.value = slot;
      var info = gamepads[slot];
      var cameraInfo = info && info.camera_id ? " [" + info.camera_id + "]" : "";
      option.textContent = slot + cameraInfo;
      gamepadSlotSelect.appendChild(option);
    });
    gamepadSlotSelect.disabled = false;
  }
}

// ---------------------------------------------------------------------------
// Fetch helper
// ---------------------------------------------------------------------------
function apiFetch(serverAddress, endpoint) {
  return fetch("http://" + serverAddress + endpoint, {
    method: "GET",
    headers: { "Accept": "application/json" }
  });
}

// ---------------------------------------------------------------------------
// Status indicator
// ---------------------------------------------------------------------------
function setStatus(status, message) {
  statusDot.className = "status-dot " + status;
  statusText.textContent = message;
  isConnected = (status === "connected");
}

// ---------------------------------------------------------------------------
// Settings persistence via Stream Deck WebSocket
// ---------------------------------------------------------------------------
function saveSettings() {
  console.log("Saving settings:", currentSettings);
  if (websocket && websocket.readyState === WebSocket.OPEN && uuid) {
    websocket.send(JSON.stringify({
      event: "setSettings",
      context: uuid,
      payload: currentSettings
    }));
    console.log("Settings saved via WebSocket");
  } else {
    console.warn("WebSocket not connected, cannot save settings");
  }
}

// ---------------------------------------------------------------------------
// Test connection (calls /api/health then refreshes dropdowns)
// ---------------------------------------------------------------------------
async function testConnection() {
  var serverAddress = currentSettings.serverAddress;
  if (!serverAddress) {
    setStatus("not-configured", "Enter server address");
    return;
  }

  testButton.disabled = true;
  testButton.textContent = "Testing...";
  setStatus("connecting", "Connecting...");

  try {
    var response = await apiFetch(serverAddress, "/api/health");

    if (response.ok) {
      setStatus("connected", "Connected");
      await window.piConfig.fetchDropdownData(serverAddress);
    } else {
      setStatus("disconnected", "Server error: " + response.status);
    }
  } catch (error) {
    console.error("Connection test failed:", error);
    setStatus("disconnected", "Connection failed");
  } finally {
    testButton.disabled = false;
    testButton.textContent = "Test Connection";
  }
}

// ---------------------------------------------------------------------------
// Event listeners (shared: server input debounce + test button)
// ---------------------------------------------------------------------------
function setupEventListeners() {
  if (listenersInitialized) return;
  listenersInitialized = true;

  // Server address input with debounce
  serverAddressInput.addEventListener("input", function() {
    currentSettings.serverAddress = this.value;

    if (debounceTimer) {
      clearTimeout(debounceTimer);
    }

    setStatus("not-configured", "Not configured");
    window.piConfig.resetDropdowns();

    debounceTimer = setTimeout(function() {
      if (currentSettings.serverAddress) {
        saveSettings();
        window.piConfig.fetchDropdownData(currentSettings.serverAddress);
      }
    }, 500);
  });

  // Test connection button
  testButton.addEventListener("click", function() {
    testConnection();
  });

  // Action-specific listeners (dropdown changes, etc.)
  window.piConfig.setupActionListeners(saveSettings);
}

// ---------------------------------------------------------------------------
// Main entry point called by Stream Deck runtime
// ---------------------------------------------------------------------------
function connectElgatoStreamDeckSocket(inPort, inPropertyInspectorUUID, inRegisterEvent, inInfo, inActionInfo) {
  console.log("connectElgatoStreamDeckSocket called", { inPort, inPropertyInspectorUUID, inRegisterEvent });

  // Grab shared DOM elements now that the page is loaded
  serverAddressInput = document.getElementById("serverAddress");
  statusDot = document.getElementById("statusDot");
  statusText = document.getElementById("statusText");
  testButton = document.getElementById("testConnection");

  uuid = inPropertyInspectorUUID;
  actionInfo = JSON.parse(inActionInfo);
  sdInfo = JSON.parse(inInfo);

  // Initialize currentSettings from action-specific defaults
  currentSettings = Object.assign({}, window.piConfig.defaultSettings);

  // Overlay persisted settings from actionInfo
  if (actionInfo && actionInfo.payload && actionInfo.payload.settings) {
    currentSettings = window.piConfig.buildSettingsFromPayload(actionInfo.payload.settings);
    console.log("Loaded settings from actionInfo:", currentSettings);
    window.piConfig.applySettingsToUI(currentSettings);
  }

  // Connect to Stream Deck
  websocket = new WebSocket("ws://127.0.0.1:" + inPort);

  websocket.onopen = function() {
    console.log("WebSocket connected to Stream Deck");
    websocket.send(JSON.stringify({
      event: inRegisterEvent,
      uuid: inPropertyInspectorUUID
    }));

    websocket.send(JSON.stringify({
      event: "getSettings",
      context: uuid
    }));
  };

  websocket.onmessage = function(evt) {
    var data = JSON.parse(evt.data);
    console.log("Received from Stream Deck:", data);

    if (data.event === "didReceiveSettings") {
      var settings = data.payload.settings || {};
      currentSettings = window.piConfig.buildSettingsFromPayload(settings);
      console.log("Applied settings from didReceiveSettings:", currentSettings);
      window.piConfig.applySettingsToUI(currentSettings);

      if (currentSettings.serverAddress) {
        window.piConfig.fetchDropdownData(currentSettings.serverAddress);
      }
    }
  };

  websocket.onerror = function(error) {
    console.error("WebSocket error:", error);
  };

  websocket.onclose = function() {
    console.log("WebSocket closed");
  };

  setupEventListeners();

  if (currentSettings.serverAddress) {
    window.piConfig.fetchDropdownData(currentSettings.serverAddress);
  }
}

// ---------------------------------------------------------------------------
// Standalone fallback for development/testing outside Stream Deck
// ---------------------------------------------------------------------------
setTimeout(function() {
  if (!websocket) {
    console.log("Running in standalone mode (no Stream Deck connection)");

    // Grab shared DOM elements
    serverAddressInput = document.getElementById("serverAddress");
    statusDot = document.getElementById("statusDot");
    statusText = document.getElementById("statusText");
    testButton = document.getElementById("testConnection");

    currentSettings = Object.assign({}, window.piConfig.defaultSettings);

    setupEventListeners();
    window.piConfig.applySettingsToUI(currentSettings);
    if (currentSettings.serverAddress) {
      window.piConfig.fetchDropdownData(currentSettings.serverAddress);
    }
  }
}, 1000);
