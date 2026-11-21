//! Every user-visible string of the native UI, in one table.  English only
//! for now; a translation layer would replace this module.

pub const PRODUCT: &str = "Open Desk Phone";

// Softkeys and actions.
pub const MORE: &str = "More...";
pub const DELETE: &str = "Delete";
pub const BACK: &str = "Back";
pub const CANCEL: &str = "Cancel";
pub const OK: &str = "OK";
pub const SELECT: &str = "Select";
pub const SAVE: &str = "Save";
pub const EDIT: &str = "Edit";
pub const YES: &str = "Yes";
pub const NO: &str = "No";
pub const EXIT: &str = "Exit";
pub const DIAL: &str = "Dial";
pub const ANSWER: &str = "Answer";
pub const IGNORE: &str = "Ignore";
pub const REJECT: &str = "Reject";
pub const VOICEMAIL: &str = "VM";
pub const END_CALL: &str = "End Call";
pub const HOLD: &str = "Hold";
pub const RESUME: &str = "Resume";
pub const TRANSFER: &str = "Transfer";
pub const CONFERENCE: &str = "Conference";
pub const SPLIT: &str = "Split";
pub const PARK: &str = "Park";
pub const NEW_CALL: &str = "New Call";
pub const CONSULT: &str = "Consult";
pub const BLIND: &str = "Blind";
pub const COMPLETE: &str = "Complete";
pub const JOIN: &str = "Join";
pub const SWAP: &str = "Swap";
pub const MENU: &str = "Menu";
pub const APPS: &str = "Apps";
pub const LOG: &str = "Log";
pub const CONTACTS: &str = "Contacts";
pub const DND: &str = "DND";
pub const DND_OFF: &str = "DND Off";
pub const MISSED_CALLS_KEY: &str = "Missed Calls";
pub const FORWARD: &str = "Forward";
pub const REDIAL: &str = "Redial";
pub const CLEAR: &str = "Clear";
pub const DETAILS: &str = "Details";
pub const RETRY: &str = "Retry";
pub const SKIP: &str = "Skip";
pub const MANUAL: &str = "Manual";
pub const REBOOT: &str = "Reboot";
pub const PLAY: &str = "Play";
pub const PAGE: &str = "Page";
pub const ON: &str = "On";
pub const OFF: &str = "Off";
pub const TOGGLE: &str = "Toggle";
pub const DISMISS: &str = "Dismiss";
pub const SNOOZE: &str = "Snooze";
pub const ALARM: &str = "Alarm";
pub const ROLLBACK: &str = "Roll back";

// Boot, recovery, provisioning.
pub const STARTING: &str = "Starting\u{2026}";
pub const STARTING_CONFIG: &str = "Reading the configuration\u{2026}";
pub const STARTING_AUDIO: &str = "Starting audio\u{2026}";
pub const STARTING_TELEPHONY: &str = "Starting telephony\u{2026}";
pub const STARTING_APPS: &str = "Starting apps\u{2026}";
pub const STARTING_WEB: &str = "Starting the web interface\u{2026}";
pub const WAITING_NETWORK: &str = "Waiting for network\u{2026}";
pub const NO_LINK: &str = "Network cable unplugged";
pub const GETTING_ADDRESS: &str = "Getting an address\u{2026}";
pub const NETWORK_FAILED: &str = "Network setup failed";
pub const SHUTTING_DOWN: &str = "Restarting\u{2026}";
pub const RECOVERY_TITLE: &str = "Recovery";
pub const RECOVERY_TEXT: &str =
    "The settings storage could not be mounted. Nothing was erased. Use the web page to repair or reset it.";
pub const PROV_TITLE: &str = "Setup";
pub const PROV_DISCOVERING: &str = "Looking for a server\u{2026}";
pub const PROV_SELECT_SERVER: &str = "Select a server";
pub const PROV_ENTER_SERVER: &str = "Server address";
pub const PROV_CONTACTING: &str = "Contacting server\u{2026}";
pub const PROV_SELECT_USER: &str = "Select your extension";
pub const PROV_ENTER_PIN: &str = "Enter PIN";
pub const PROV_FETCHING: &str = "Fetching configuration\u{2026}";
pub const PROV_APPLYING: &str = "Applying configuration\u{2026}";
pub const PROV_FAILED: &str = "Setup failed";
pub const PROV_ENTER_MANUALLY: &str = "Enter address\u{2026}";
pub const PROV_NO_SERVER: &str = "Use without a server";

// Idle.
pub const NO_ACCOUNTS: &str = "No accounts configured";
pub const NOT_REGISTERED: &str = "Not registered";
pub const REGISTERING: &str = "Registering\u{2026}";
pub const DND_ON: &str = "Do Not Disturb";
pub const FORWARD_ON: &str = "Forwarding to";
pub const MISSED_CALLS: &str = "missed";
pub const NEW_MESSAGES: &str = "new";
pub const UNUSED: &str = "";

// Dialer and calls.
pub const NEW_CALL_TITLE: &str = "New Call";
pub const TRANSFER_TO: &str = "Transfer to";
pub const CONFERENCE_WITH: &str = "Conference with";
pub const INCOMING_CALL: &str = "Incoming";
pub const CALLING: &str = "Calling";
pub const RINGING: &str = "Ringing\u{2026}";
pub const CONNECTED: &str = "Connected";
pub const ON_HOLD: &str = "On hold";
pub const HELD_BY_REMOTE: &str = "Held by other party";
pub const TRANSFERRING: &str = "Transferring\u{2026}";
pub const IN_CONFERENCE: &str = "Conference";
pub const CALL_ENDED: &str = "Call ended";
pub const CALL_FAILED: &str = "Call failed";
pub const BUSY: &str = "Busy";
pub const NO_ANSWER: &str = "No answer";
pub const REJECTED: &str = "Declined";
pub const MUTED: &str = "Muted";
pub const UNKNOWN_CALLER: &str = "Unknown";
pub const PAGING: &str = "Paging";
pub const SECURE: &str = "Secure";

// Call log.
pub const CALL_LOG: &str = "Call Log";
pub const ALL: &str = "All";
pub const MISSED: &str = "Missed";
pub const RECEIVED: &str = "Received";
pub const PLACED: &str = "Placed";
pub const NO_CALLS: &str = "No calls";
pub const CLEAR_LOG_Q: &str = "Clear the call log?";

// Menu and settings.
pub const MAIN_MENU: &str = "Menu";
pub const APPLICATIONS: &str = "Applications";
pub const NO_APPS: &str = "No applications";
pub const CALL_FORWARD: &str = "Call Forward";
pub const FWD_ALWAYS: &str = "Always";
pub const FWD_BUSY: &str = "When busy";
pub const FWD_NO_ANSWER: &str = "No answer";
pub const FWD_NUMBER: &str = "Forward to";
pub const DO_NOT_DISTURB: &str = "Do Not Disturb";
pub const SETTINGS: &str = "Settings";
pub const NETWORK: &str = "Network";
pub const ACCOUNTS: &str = "Accounts";
pub const DISPLAY: &str = "Display";
pub const SOUNDS: &str = "Sounds";
pub const RESET: &str = "Reset";
pub const ABOUT: &str = "About";
pub const STATUS: &str = "Status";
pub const BRIGHTNESS: &str = "Brightness";
pub const CONTRAST: &str = "Contrast";
pub const DIM_BACKLIGHT: &str = "Dim when idle";
pub const BACKLIGHT_TIMEOUT: &str = "Backlight timeout";
pub const RINGTONE: &str = "Ringtone";
pub const THEME: &str = "Theme";
pub const RINGER_VOLUME: &str = "Ringer volume";
pub const ADDRESSING: &str = "Addressing";
pub const DHCP: &str = "DHCP";
pub const STATIC: &str = "Static";
pub const IP_ADDRESS: &str = "IP address";
pub const SUBNET_MASK: &str = "Subnet mask";
pub const GATEWAY: &str = "Gateway";
pub const DNS1: &str = "DNS 1";
pub const DNS2: &str = "DNS 2";
pub const DNS: &str = "DNS";
pub const VLAN: &str = "VLAN";
pub const VLAN_MODE: &str = "VLAN discovery";
pub const VLAN_ID: &str = "VLAN ID";
pub const VLAN_NONE: &str = "None";
pub const VLAN_LLDP: &str = "LLDP";
pub const VLAN_MANUAL: &str = "Manual";
pub const APPLY_NETWORK: &str = "Apply and restart network";
pub const RESTART_PHONE: &str = "Restart phone";
pub const FACTORY_RESET: &str = "Factory reset";
pub const REPROVISION: &str = "Reconfigure from server";
pub const FORGET_SERVER: &str = "Forget server";
pub const RESTART_Q: &str = "Restart the phone?";
pub const FACTORY_RESET_Q: &str = "Erase all settings and restart?";
pub const FORGET_SERVER_Q: &str = "Forget the server and set up again?";
pub const MODEL: &str = "Model";
pub const VERSION: &str = "Version";
pub const MAC: &str = "MAC";
pub const HOSTNAME: &str = "Hostname";
pub const LINK: &str = "LAN port";
pub const PC_PORT: &str = "PC port";
pub const LINK_DOWN: &str = "Down";
pub const SERVER: &str = "Server";
pub const USER: &str = "User";
pub const REGISTERED: &str = "Registered";
pub const DISABLED: &str = "Disabled";
pub const BANK: &str = "Firmware bank";
pub const UPTIME: &str = "Uptime";
pub const MEMORY: &str = "Memory";
pub const DSP: &str = "DSP";
pub const TIME: &str = "Time";
pub const NOT_SET: &str = "Not set";
pub const LAST_REBOOT: &str = "Last restart";
pub const PROVISIONING: &str = "Provisioning";
pub const HEALTH: &str = "Subsystems";

// Paging.
pub const PAGING_TITLE: &str = "Paging";
pub const PAGE_ACTIVE: &str = "Page in progress";
pub const PAGE_RECEIVING: &str = "Page from";
pub const PAGE_SENDING: &str = "Paging";
pub const PAGE_BLOCKED: &str = "Page waiting";
pub const PAGE_STOP: &str = "Stop";
pub const NO_PAGE_GROUPS: &str = "No page groups";

// Volume.
pub const VOL_HANDSET: &str = "Handset";
pub const VOL_HEADSET: &str = "Headset";
pub const VOL_SPEAKER: &str = "Speaker";
pub const VOL_RINGER: &str = "Ringer";

// Update.
pub const UPDATE_TITLE: &str = "Firmware Update";
pub const UPDATE_WARNING: &str = "Do not unplug the phone";
pub const UPD_RECEIVING: &str = "Receiving";
pub const UPD_CHECKING: &str = "Checking image";
pub const UPD_ERASING: &str = "Erasing";
pub const UPD_KERNEL: &str = "Writing kernel";
pub const UPD_ROOTFS: &str = "Writing system";
pub const UPD_VERIFYING: &str = "Verifying";
pub const UPD_COMMITTING: &str = "Finishing";
pub const UPD_COMPLETE: &str = "Update complete; restarting";
pub const UPD_FAILED: &str = "Update failed";
/// The new image runs and its trial is not yet confirmed.
pub const UPD_TRIAL: &str = "Firmware update in progress...";
pub const TRIAL_PENDING: &str = "New firmware on trial";
pub const ROLLED_BACK: &str = "Firmware was rolled back";

pub const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
pub const MONTHS: [&str; 12] =
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

// Bluetooth.
pub const BLUETOOTH: &str = "Bluetooth";
pub const BT_NO_CONTROLLER: &str = "No Bluetooth controller";
pub const BT_SEARCHING: &str = "Searching";
pub const BT_LOOKING: &str = "Looking for devices";
pub const BT_NONE_FOUND: &str = "No devices found";
pub const BT_PAIRING_MODE: &str = "Put the headset in pairing mode.";
pub const BT_PAIRING_MODE_SEARCH: &str = "Put the headset in pairing mode, then press Search.";
pub const BT_PAIRED_DEVICES: &str = "Paired devices";
pub const BT_AVAILABLE_DEVICES: &str = "Available devices";
pub const BT_CONNECTED: &str = "Connected";
pub const BT_NOT_CONNECTED: &str = "Not connected";
pub const BT_PAIRING: &str = "Pairing\u{2026}";
pub const BT_PAIR: &str = "Pair";
pub const BT_SEARCH: &str = "Search";
pub const BT_CONNECT: &str = "Connect";
pub const BT_DISCONNECT: &str = "Disconnect";
pub const BT_FORGET: &str = "Forget";
pub const BT_PAIR_WITH: &str = "Pair with";
pub const BT_COMPARE: &str = "Pair if the headset shows this number.";
pub const BT_ASKS: &str = "This device asks to pair with the phone.";
pub const BT_TYPE_PIN: &str = "Type the headset's PIN, which is often 0000.";

// Touch (D80): the idle screen, the dialer, and the call screen.
pub const KEYPAD: &str = "Keypad";
pub const HIDE_KEYPAD: &str = "Hide Keypad";
pub const MUTE: &str = "Mute";
pub const LINE_AVAILABLE: &str = "Available";
pub const LINE_UNAVAILABLE: &str = "Unavailable";
