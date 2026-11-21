//! The audio gateway's side of the AT command dialogue of the Hands-Free
//! profile (without three-way calling or voice recognition) and of the
//! Headset profile.
//!
//! Voice is narrow-band (CVSD) unless the gateway is made with
//! [`Gateway::with_wideband`] and the headset negotiates codecs too: then
//! the two agree on a codec before each voice connection (`+BCS`), and
//! wide-band speech (mSBC, 16 kHz) is chosen when the headset has it.
//!
//! Pure protocol: bytes from the RFCOMM link go in through
//! [`Gateway::receive`], bytes to send come out of [`Gateway::take_output`],
//! and what the headset asked for comes out as [`Action`]s.

use std::time::{Duration, Instant};

/// Gateway feature bits for `+BRSF` and the service record: bit 5, "ability
/// to reject a call".
pub const AG_FEATURES: u16 = 1 << 5;
/// Gateway feature bit 9 of `+BRSF`: codec negotiation.
pub const AG_CODEC_NEGOTIATION: u16 = 1 << 9;
/// Feature bit 7 of the headset's `AT+BRSF`: codec negotiation.
const HF_CODEC_NEGOTIATION: u32 = 1 << 7;
/// Bit 5 of the service record's feature attribute: wide-band speech.  (The
/// attribute's bits are not `+BRSF`'s: its first five are, and this is its
/// sixth.)
pub const SDP_WIDEBAND_SPEECH: u16 = 1 << 5;

/// A voice codec, by its number in `+BCS` and `AT+BAC`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    /// Narrow-band, 8 kHz.  Every headset has it.
    Cvsd = 1,
    /// Wide-band speech, 16 kHz.
    Msbc = 2,
}

/// How often `RING` repeats while a call alerts.
pub const RING_INTERVAL: Duration = Duration::from_secs(3);

/// The phone's call state, as far as a headset is told.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum CallState {
    #[default]
    Idle,
    /// A call is alerting; `number` is the caller, if known.
    Incoming { number: Option<String> },
    /// An outgoing call is being set up.
    Dialing,
    /// The far end of an outgoing call is ringing.
    Alerting,
    Active,
}

/// Something the headset asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// The service level connection is up: the headset is usable.
    Connected,
    Answer,
    /// Reject the alerting call or end the current one.
    HangUp,
    Dial(String),
    Redial,
    Dtmf(char),
    /// Earpiece volume, 0 to 15.
    SpeakerVolume(u8),
    /// Microphone gain, 0 to 15.
    MicVolume(u8),
    /// The single button of a Headset-profile device.
    Button,
    /// The headset asks the gateway to open the voice connection
    /// (`AT+BCC`), which with codec negotiation only the gateway does.
    OpenAudio,
    /// The headset agreed to the codec offered with
    /// [`Gateway::offer_codec`]: the voice connection can be opened.
    CodecSelected(Codec),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    HandsFree,
    Headset,
}

// Indicator order of the +CIND reply, numbered from 1 in +CIEV.
const IND_SERVICE: usize = 0;
const IND_CALL: usize = 1;
const IND_CALLSETUP: usize = 2;
const IND_CALLHELD: usize = 3;
const IND_SIGNAL: usize = 4;
const IND_ROAM: usize = 5;
const IND_BATTCHG: usize = 6;
const CIND_TEST: &str = "+CIND: (\"service\",(0,1)),(\"call\",(0,1)),(\"callsetup\",(0-3)),(\"callheld\",(0-2)),(\"signal\",(0-5)),(\"roam\",(0,1)),(\"battchg\",(0-5))";

pub struct Gateway {
    dialect: Dialect,
    line: Vec<u8>,
    out: Vec<u8>,
    actions: Vec<Action>,
    /// Service level connection established.
    connected: bool,
    /// The headset asked for indicator events (`AT+CMER`).
    events: bool,
    /// The headset asked for caller ID (`AT+CLIP=1`).
    clip: bool,
    indicators: [u8; 7],
    state: CallState,
    next_ring: Option<Instant>,
    /// This gateway negotiates codecs and has wide-band speech.
    wideband: bool,
    /// The headset negotiates codecs (its `AT+BRSF`).
    hf_negotiates: bool,
    /// The headset has mSBC (its `AT+BAC`).
    hf_msbc: bool,
    /// The codec offered with `+BCS` and not yet answered.
    offered: Option<Codec>,
}

impl Gateway {
    pub fn new(dialect: Dialect) -> Gateway {
        let mut indicators = [0u8; 7];
        // Wired phone: always in service, full signal, full battery.
        indicators[IND_SERVICE] = 1;
        indicators[IND_SIGNAL] = 5;
        indicators[IND_BATTCHG] = 5;
        let _ = (IND_CALLHELD, IND_ROAM);
        Gateway {
            dialect,
            line: Vec::new(),
            out: Vec::new(),
            actions: Vec::new(),
            // The Headset profile has no set-up dialogue.
            connected: dialect == Dialect::Headset,
            events: false,
            clip: false,
            indicators,
            state: CallState::Idle,
            next_ring: None,
            wideband: false,
            hf_negotiates: false,
            hf_msbc: false,
            offered: None,
        }
    }

    /// The gateway of a phone whose Bluetooth voice can be wide-band: it
    /// negotiates codecs with a headset that does.
    pub fn with_wideband(mut self, wideband: bool) -> Gateway {
        self.wideband = wideband && self.dialect == Dialect::HandsFree;
        self
    }

    /// The feature bits of this gateway's `+BRSF`.
    pub fn features(&self) -> u16 {
        AG_FEATURES | if self.wideband { AG_CODEC_NEGOTIATION } else { 0 }
    }

    /// Whether a codec is agreed on before each voice connection: when
    /// both sides negotiate codecs.
    pub fn negotiates(&self) -> bool {
        self.wideband && self.hf_negotiates
    }

    /// The best codec both sides have.
    pub fn best_codec(&self) -> Codec {
        if self.negotiates() && self.hf_msbc { Codec::Msbc } else { Codec::Cvsd }
    }

    /// Offers the headset a codec for the voice connection (`+BCS`).  Its
    /// agreement comes as [`Action::CodecSelected`].
    pub fn offer_codec(&mut self, codec: Codec) {
        self.offered = Some(codec);
        self.send(&format!("+BCS: {}", codec as u8));
    }

    pub fn is_connected(&self) -> bool {
        self.connected
    }

    /// Bytes to write to the link.
    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    pub fn take_actions(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.actions)
    }

    /// When [`Gateway::tick`] next has something to do.
    pub fn deadline(&self) -> Option<Instant> {
        self.next_ring
    }

    fn send(&mut self, text: &str) {
        self.out.extend_from_slice(b"\r\n");
        self.out.extend_from_slice(text.as_bytes());
        self.out.extend_from_slice(b"\r\n");
    }

    fn ok(&mut self) {
        self.send("OK");
    }

    fn set_indicator(&mut self, index: usize, value: u8) {
        if self.indicators[index] == value {
            return;
        }
        self.indicators[index] = value;
        if self.connected && self.events && self.dialect == Dialect::HandsFree {
            self.send(&format!("+CIEV: {},{}", index + 1, value));
        }
    }

    fn ring(&mut self) {
        self.send("RING");
        if self.clip
            && let CallState::Incoming { number: Some(number) } = &self.state
        {
            // Type 129: unknown numbering plan; the number is a dial string.
            let number: String = number.chars().filter(|c| *c != '"').collect();
            self.send(&format!("+CLIP: \"{number}\",129"));
        }
    }

    /// Tell the headset about the phone's call state.
    pub fn set_call_state(&mut self, now: Instant, state: CallState) {
        if state == self.state {
            return;
        }
        let (call, setup) = match &state {
            CallState::Idle => (0, 0),
            CallState::Incoming { .. } => (0, 1),
            CallState::Dialing => (0, 2),
            CallState::Alerting => (0, 3),
            CallState::Active => (1, 0),
        };
        self.state = state;
        // Answering: "call" goes to 1 before "callsetup" returns to 0.
        self.set_indicator(IND_CALL, call);
        self.set_indicator(IND_CALLSETUP, setup);
        if matches!(self.state, CallState::Incoming { .. }) && self.connected {
            self.ring();
            self.next_ring = Some(now + RING_INTERVAL);
        } else {
            self.next_ring = None;
        }
    }

    /// Repeat `RING` when it is due.
    pub fn tick(&mut self, now: Instant) {
        if let Some(at) = self.next_ring
            && now >= at
        {
            self.ring();
            self.next_ring = Some(now + RING_INTERVAL);
        }
    }

    /// Tell the headset the earpiece volume changed on the phone (0 to 15).
    pub fn set_speaker_volume(&mut self, level: u8) {
        if self.connected {
            self.send(&format!("+VGS: {}", level.min(15)));
        }
    }

    /// Feed bytes received from the link.
    pub fn receive(&mut self, data: &[u8]) {
        for &b in data {
            match b {
                b'\r' => {
                    let line = std::mem::take(&mut self.line);
                    let text = String::from_utf8_lossy(&line).trim().to_string();
                    if !text.is_empty() {
                        self.command(&text);
                    }
                }
                b'\n' => {}
                _ if self.line.len() < 512 => self.line.push(b),
                _ => {}
            }
        }
    }

    fn established(&mut self) {
        if !self.connected {
            self.connected = true;
            self.actions.push(Action::Connected);
            if matches!(self.state, CallState::Incoming { .. }) {
                self.ring();
                self.next_ring = Some(Instant::now() + RING_INTERVAL);
            }
        }
    }

    fn command(&mut self, line: &str) {
        let upper = line.to_ascii_uppercase();
        let Some(body) = upper.strip_prefix("AT") else {
            self.send("ERROR");
            return;
        };
        // Volume and the headset button are common to both profiles.
        if let Some(v) = body.strip_prefix("+VGS=") {
            return self.level(v, Action::SpeakerVolume);
        }
        if let Some(v) = body.strip_prefix("+VGM=") {
            return self.level(v, Action::MicVolume);
        }
        if body.starts_with("+CKPD=") {
            self.actions.push(Action::Button);
            return self.ok();
        }
        if self.dialect == Dialect::Headset {
            return self.send("ERROR");
        }

        if let Some(v) = body.strip_prefix("+BRSF=") {
            self.hf_negotiates = v.trim().parse::<u32>().is_ok_and(|f| f & HF_CODEC_NEGOTIATION != 0);
            self.send(&format!("+BRSF: {}", self.features()));
            self.ok();
        } else if let Some(list) = body.strip_prefix("+BAC=").filter(|_| self.wideband) {
            // The codecs the headset has now.  It may send this again,
            // e.g., to take one back.
            self.hf_msbc = list.split(',').any(|c| c.trim() == "2");
            self.ok();
        } else if body == "+BCC" && self.negotiates() {
            self.ok();
            self.actions.push(Action::OpenAudio);
        } else if let Some(v) = body.strip_prefix("+BCS=").filter(|_| self.offered.is_some()) {
            match self.offered.take() {
                Some(codec) if v.trim().parse() == Ok(codec as u8) => {
                    self.ok();
                    self.actions.push(Action::CodecSelected(codec));
                }
                _ => self.send("ERROR"),
            }
        } else if body == "+CIND=?" {
            self.send(CIND_TEST);
            self.ok();
        } else if body == "+CIND?" {
            let values: Vec<String> = self.indicators.iter().map(u8::to_string).collect();
            self.send(&format!("+CIND: {}", values.join(",")));
            self.ok();
        } else if let Some(args) = body.strip_prefix("+CMER=") {
            // "3,0,0,1" switches indicator events on, "3,0,0,0" off.
            let fields: Vec<&str> = args.split(',').map(str::trim).collect();
            self.events = fields.first() == Some(&"3") && fields.get(3) == Some(&"1");
            self.ok();
            // Without three-way calling on our side, the dialogue ends here.
            self.established();
        } else if let Some(v) = body.strip_prefix("+CLIP=") {
            self.clip = v.trim() == "1";
            self.ok();
        } else if ["+CCWA=", "+CMEE=", "+BIA=", "+COPS="].iter().any(|p| body.starts_with(p)) {
            // Settings with nothing to act on here: accepted.
            self.ok();
        } else if body == "+COPS?" {
            self.send("+COPS: 0,0,\"Open Desk Phone\"");
            self.ok();
        } else if body == "+CLCC" {
            let line = match &self.state {
                CallState::Idle => None,
                CallState::Incoming { .. } => Some("+CLCC: 1,1,4,0,0"),
                CallState::Dialing => Some("+CLCC: 1,0,2,0,0"),
                CallState::Alerting => Some("+CLCC: 1,0,3,0,0"),
                CallState::Active => Some("+CLCC: 1,0,0,0,0"),
            };
            if let Some(line) = line {
                self.send(line);
            }
            self.ok();
        } else if body == "+CNUM" || body == "+BTRH?" {
            self.ok();
        } else if body == "A" {
            self.ok();
            self.actions.push(Action::Answer);
        } else if body == "+CHUP" {
            self.ok();
            self.actions.push(Action::HangUp);
        } else if body == "+BLDN" {
            self.ok();
            self.actions.push(Action::Redial);
        } else if let Some(rest) = body.strip_prefix("D") {
            // "ATD<number>;" places a voice call; memory dialing ("ATD>n;")
            // has no meaning here.
            let number = rest.trim_end_matches(';').trim();
            let valid = !number.is_empty()
                && number.chars().all(|c| c.is_ascii_digit() || matches!(c, '+' | '*' | '#' | 'A'..='D'));
            if valid && rest.ends_with(';') {
                self.ok();
                self.actions.push(Action::Dial(number.to_string()));
            } else {
                self.send("ERROR");
            }
        } else if let Some(d) = body.strip_prefix("+VTS=") {
            match d.trim().chars().next() {
                Some(c) if c.is_ascii_digit() || matches!(c, '*' | '#' | 'A'..='D') => {
                    self.ok();
                    self.actions.push(Action::Dtmf(c));
                }
                _ => self.send("ERROR"),
            }
        } else {
            // Everything optional that we do not offer (three-way calling,
            // voice recognition, echo control, vendor extensions, and
            // without wide-band speech, codec negotiation).
            self.send("ERROR");
        }
    }

    fn level(&mut self, text: &str, action: fn(u8) -> Action) {
        match text.trim().parse::<u8>() {
            Ok(v) if v <= 15 => {
                self.ok();
                self.actions.push(action(v));
            }
            _ => self.send("ERROR"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(g: &mut Gateway) -> String {
        String::from_utf8(g.take_output()).unwrap()
    }

    fn lines(g: &mut Gateway) -> Vec<String> {
        text(g).split("\r\n").filter(|l| !l.is_empty()).map(str::to_string).collect()
    }

    fn connect(g: &mut Gateway) {
        g.receive(b"AT+BRSF=27\rAT+CIND=?\rAT+CIND?\rAT+CMER=3,0,0,1\r");
    }

    #[test]
    fn service_level_connection() {
        let mut g = Gateway::new(Dialect::HandsFree);
        assert!(!g.is_connected());
        connect(&mut g);
        assert_eq!(
            lines(&mut g),
            ["+BRSF: 32", "OK", CIND_TEST, "OK", "+CIND: 1,0,0,0,5,0,5", "OK", "OK"]
        );
        assert!(g.is_connected());
        assert_eq!(g.take_actions(), [Action::Connected]);
    }

    #[test]
    fn responses_are_framed_with_cr_lf() {
        let mut g = Gateway::new(Dialect::HandsFree);
        g.receive(b"AT+CLIP=1\r");
        assert_eq!(text(&mut g), "\r\nOK\r\n");
    }

    #[test]
    fn incoming_call_rings_with_caller_id_and_is_answered() {
        let t0 = Instant::now();
        let mut g = Gateway::new(Dialect::HandsFree);
        connect(&mut g);
        g.receive(b"AT+CLIP=1\r");
        g.take_output();
        g.take_actions();

        g.set_call_state(t0, CallState::Incoming { number: Some("1234".into()) });
        assert_eq!(lines(&mut g), ["+CIEV: 3,1", "RING", "+CLIP: \"1234\",129"]);
        assert_eq!(g.deadline(), Some(t0 + RING_INTERVAL));

        g.tick(t0 + Duration::from_secs(1));
        assert!(text(&mut g).is_empty());
        g.tick(t0 + RING_INTERVAL);
        assert_eq!(lines(&mut g), ["RING", "+CLIP: \"1234\",129"]);

        g.receive(b"ATA\r");
        assert_eq!(lines(&mut g), ["OK"]);
        assert_eq!(g.take_actions(), [Action::Answer]);

        g.set_call_state(t0, CallState::Active);
        assert_eq!(lines(&mut g), ["+CIEV: 2,1", "+CIEV: 3,0"]);
        assert_eq!(g.deadline(), None);

        g.receive(b"AT+CHUP\r");
        assert_eq!(g.take_actions(), [Action::HangUp]);
        g.take_output();
        g.set_call_state(t0, CallState::Idle);
        assert_eq!(lines(&mut g), ["+CIEV: 2,0"]);
    }

    #[test]
    fn outgoing_call_indicators() {
        let t0 = Instant::now();
        let mut g = Gateway::new(Dialect::HandsFree);
        connect(&mut g);
        g.take_output();
        g.set_call_state(t0, CallState::Dialing);
        g.set_call_state(t0, CallState::Alerting);
        g.set_call_state(t0, CallState::Active);
        assert_eq!(lines(&mut g), ["+CIEV: 3,2", "+CIEV: 3,3", "+CIEV: 2,1", "+CIEV: 3,0"]);
    }

    #[test]
    fn no_indicator_events_before_they_are_asked_for() {
        let t0 = Instant::now();
        let mut g = Gateway::new(Dialect::HandsFree);
        g.receive(b"AT+BRSF=0\rAT+CIND=?\rAT+CIND?\r");
        g.take_output();
        g.set_call_state(t0, CallState::Incoming { number: None });
        assert!(text(&mut g).is_empty());
        // The state is reported by +CIND?, and ringing starts on connection.
        g.receive(b"AT+CIND?\rAT+CMER=3,0,0,1\r");
        assert_eq!(lines(&mut g), ["+CIND: 1,0,1,0,5,0,5", "OK", "OK", "RING"]);
    }

    #[test]
    fn dialing_redial_dtmf_and_volume() {
        let mut g = Gateway::new(Dialect::HandsFree);
        connect(&mut g);
        g.take_output();
        g.take_actions();
        g.receive(b"ATD5551234;\rAT+BLDN\rAT+VTS=5\rAT+VGS=11\rAT+VGM=3\rAT+VGS=16\rATD>1;\rATD123\r");
        assert_eq!(lines(&mut g), ["OK", "OK", "OK", "OK", "OK", "ERROR", "ERROR", "ERROR"]);
        assert_eq!(
            g.take_actions(),
            [
                Action::Dial("5551234".into()),
                Action::Redial,
                Action::Dtmf('5'),
                Action::SpeakerVolume(11),
                Action::MicVolume(3),
            ]
        );
        g.set_speaker_volume(9);
        assert_eq!(lines(&mut g), ["+VGS: 9"]);
    }

    #[test]
    fn unsupported_and_garbage_commands_get_error() {
        let mut g = Gateway::new(Dialect::HandsFree);
        g.receive(b"AT+BVRA=1\rAT+NREC=0\rAT+CHLD=?\rAT+BAC=1,2\rhello\r\r\n");
        assert_eq!(lines(&mut g), ["ERROR", "ERROR", "ERROR", "ERROR", "ERROR"]);
        assert!(g.take_actions().is_empty());
    }

    #[test]
    fn commands_split_across_reads_and_lower_case() {
        let mut g = Gateway::new(Dialect::HandsFree);
        g.receive(b"at+cl");
        g.receive(b"ip=1");
        assert!(text(&mut g).is_empty());
        g.receive(b"\r");
        assert_eq!(lines(&mut g), ["OK"]);
    }

    #[test]
    fn headset_profile() {
        let t0 = Instant::now();
        let mut g = Gateway::new(Dialect::Headset);
        assert!(g.is_connected());
        g.set_call_state(t0, CallState::Incoming { number: Some("1234".into()) });
        assert_eq!(lines(&mut g), ["RING"]);
        g.receive(b"AT+CKPD=200\rAT+VGS=7\rAT+BRSF=0\r");
        assert_eq!(lines(&mut g), ["OK", "OK", "ERROR"]);
        assert_eq!(g.take_actions(), [Action::Button, Action::SpeakerVolume(7)]);
        g.set_call_state(t0, CallState::Active);
        assert!(text(&mut g).is_empty());
    }

    /// A gateway with wide-band speech and a headset that negotiates
    /// codecs: the headset names its codecs, and each voice connection
    /// starts with the gateway's offer and the headset's agreement.
    #[test]
    fn codecs_are_negotiated_for_wideband_speech() {
        let mut g = Gateway::new(Dialect::HandsFree).with_wideband(true);
        // Headset features 0x9f: bit 7 is codec negotiation.
        g.receive(b"AT+BRSF=159\rAT+BAC=1,2\rAT+CIND=?\rAT+CIND?\rAT+CMER=3,0,0,1\r");
        assert_eq!(lines(&mut g)[..3], ["+BRSF: 544", "OK", "OK"]);
        assert!(g.negotiates());
        assert_eq!(g.best_codec(), Codec::Msbc);
        // The headset asks for the voice connection.
        g.take_actions();
        g.receive(b"AT+BCC\r");
        assert_eq!(g.take_actions(), [Action::OpenAudio]);
        assert_eq!(lines(&mut g), ["OK"]);
        g.offer_codec(Codec::Msbc);
        assert_eq!(lines(&mut g), ["+BCS: 2"]);
        // Another codec than the one offered is refused; the offer is
        // then over.
        g.receive(b"AT+BCS=1\r");
        assert_eq!((lines(&mut g), g.take_actions()), (vec!["ERROR".to_string()], vec![]));
        g.offer_codec(Codec::Msbc);
        g.receive(b"AT+BCS=2\r");
        assert_eq!(lines(&mut g), ["+BCS: 2", "OK"]);
        assert_eq!(g.take_actions(), [Action::CodecSelected(Codec::Msbc)]);
        // The headset takes mSBC back.
        g.receive(b"AT+BAC=1\r");
        assert_eq!(g.best_codec(), Codec::Cvsd);
    }

    /// Without both sides negotiating, voice is narrow-band and nothing is
    /// offered: a headset of the older profile, and a gateway without
    /// wide-band speech.
    #[test]
    fn no_negotiation_unless_both_sides_have_it() {
        let mut g = Gateway::new(Dialect::HandsFree).with_wideband(true);
        g.receive(b"AT+BRSF=27\rAT+BCC\r");
        assert_eq!(lines(&mut g), ["+BRSF: 544", "OK", "ERROR"]);
        assert!(!g.negotiates());
        assert_eq!(g.best_codec(), Codec::Cvsd);
        let mut g = Gateway::new(Dialect::HandsFree);
        g.receive(b"AT+BRSF=159\rAT+BAC=1,2\rAT+BCS=2\r");
        assert_eq!(lines(&mut g), ["+BRSF: 32", "OK", "ERROR", "ERROR"]);
        assert!(!g.negotiates());
        // The Headset profile has no such dialogue.
        assert!(!Gateway::new(Dialect::Headset).with_wideband(true).negotiates());
    }
}
