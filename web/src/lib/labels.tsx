// Display names for values the phone reports as identifiers.  Every name is
// written out here; nothing is derived from an identifier by changing its
// case, which would turn SSH into "Ssh".  A value with no entry is shown as
// the raw identifier in the monospace font, so a missing entry is visible.

import type { ReactNode } from "react";
import { Codec, EndCause, type Call } from "../gen/opendeskphone/v1/calls_pb";
import { Bank, UpdatePhase } from "../gen/opendeskphone/v1/firmware_pb";
import { Model } from "../gen/opendeskphone/v1/model_pb";
import { Ipv4Source, NetPhase, VlanSource } from "../gen/opendeskphone/v1/network_pb";
import { DiscoverySource, ProvisionPhase } from "../gen/opendeskphone/v1/provisioning_pb";
import { Subsystem, SystemPhase } from "../gen/opendeskphone/v1/status_pb";

type Names = Record<string | number, string>;

/// The display name of `value`, or the raw identifier in monospace.
export function label(names: Names, value: string | number | undefined, raw?: string): ReactNode {
  if (value === undefined || value === "") return "";
  return names[value] ?? <code className="font-mono text-[0.9em]">{raw ?? String(value)}</code>;
}

const modelNames: Record<number, string> = {
  [Model.D40]: "D40",
  [Model.D45]: "D45",
  [Model.D50]: "D50",
  [Model.D60]: "D60",
  [Model.D62]: "D62",
  [Model.D65]: "D65",
  [Model.D70]: "D70",
  [Model.D80]: "D80",
};

/// A model's name ("D60"); empty for none.
export function modelName(model: Model | undefined): string {
  return model === undefined ? "" : (modelNames[model] ?? "");
}

export const subsystemNames: Names = {
  [Subsystem.CORE]: "Core",
  [Subsystem.CONFIG]: "Configuration",
  [Subsystem.NET]: "Network",
  [Subsystem.SIP]: "SIP",
  [Subsystem.MEDIA]: "Media",
  [Subsystem.DPMA]: "DPMA",
  [Subsystem.UI]: "Phone UI",
  [Subsystem.APPS]: "Apps",
  [Subsystem.WEB]: "Web",
  [Subsystem.SSH]: "SSH",
  [Subsystem.UPDATE]: "Firmware",
};

export const systemPhaseNames: Names = {
  [SystemPhase.STARTING]: "Starting",
  [SystemPhase.NETWORK_WAIT]: "Waiting for network",
  [SystemPhase.PROVISIONING]: "Provisioning",
  [SystemPhase.RUNNING]: "Running",
  [SystemPhase.RECOVERY]: "Recovery",
  [SystemPhase.SHUTTING_DOWN]: "Shutting down",
};

export const netPhaseNames: Names = {
  [NetPhase.DOWN]: "Down",
  [NetPhase.CONFIGURING]: "Configuring",
  [NetPhase.UP]: "Up",
  [NetPhase.FAILED]: "Failed",
};

export const provisionPhaseNames: Names = {
  [ProvisionPhase.IDLE]: "Idle",
  [ProvisionPhase.DISCOVERING]: "Discovering",
  [ProvisionPhase.AWAITING_SERVER]: "Waiting for server",
  [ProvisionPhase.HANDSHAKING]: "Connecting",
  [ProvisionPhase.AWAITING_USER]: "Waiting for user",
  [ProvisionPhase.FETCHING_CONFIG]: "Downloading configuration",
  [ProvisionPhase.APPLYING]: "Applying",
  [ProvisionPhase.PROVISIONED]: "Provisioned",
  [ProvisionPhase.STANDALONE]: "Standalone",
  [ProvisionPhase.FAILED]: "Failed",
};

export const updatePhaseNames: Names = {
  [UpdatePhase.IDLE]: "Idle",
  [UpdatePhase.RECEIVING]: "Uploading",
  [UpdatePhase.CHECKING]: "Validating",
  [UpdatePhase.ERASING]: "Erasing",
  [UpdatePhase.WRITING_KERNEL]: "Writing kernel",
  [UpdatePhase.WRITING_ROOTFS]: "Writing root filesystem",
  [UpdatePhase.VERIFYING]: "Verifying",
  [UpdatePhase.COMMITTING]: "Finishing",
  [UpdatePhase.COMPLETE]: "Complete",
  [UpdatePhase.FAILED]: "Failed",
};

/// The firmware banks.
export const bankNames: Names = { [Bank.A]: "A", [Bank.B]: "B" };
/// Where the IPv4 address came from.
export const addressSourceNames: Names = { [Ipv4Source.DHCP]: "DHCP", [Ipv4Source.STATIC]: "Static" };
/// Where the voice VLAN came from.
export const vlanSourceNames: Names = { [VlanSource.CONFIG]: "Manual", [VlanSource.LLDP]: "LLDP-MED", [VlanSource.DHCP]: "DHCP" };
/// How a provisioning server was found.
export const discoverySourceNames: Names = {
  [DiscoverySource.SAVED]: "Saved",
  [DiscoverySource.DHCP]: "DHCP option 66",
  [DiscoverySource.MDNS]: "mDNS",
  [DiscoverySource.MANUAL]: "Manual",
};
export const codecNames: Names = {
  [Codec.PCMU]: "G.711 µ-law",
  [Codec.PCMA]: "G.711 A-law",
  [Codec.G722]: "G.722",
  [Codec.G726_32]: "G.726",
  [Codec.G729]: "G.729",
  [Codec.ILBC]: "iLBC",
  [Codec.OPUS]: "Opus",
  [Codec.L16]: "L16",
  [Codec.L16_WIDE]: "L16 wideband",
};

const endCauseNames: Names = {
  [EndCause.LOCAL_HANGUP]: "Hung up",
  [EndCause.REMOTE_HANGUP]: "Remote party hung up",
  [EndCause.REJECTED]: "Declined",
  [EndCause.CANCELED]: "Canceled",
  [EndCause.ANSWERED_ELSEWHERE]: "Answered elsewhere",
  [EndCause.BUSY]: "Busy",
  [EndCause.NO_ANSWER]: "No answer",
  [EndCause.TRANSFERRED]: "Transferred",
  [EndCause.FAILED]: "Failed",
  [EndCause.ERROR]: "Failed",
};

/// Why a call ended: the SIP status of a failure, the text of an error,
/// or else the cause's name.  Empty while the call is up.
export function endCause(call: Call): ReactNode {
  if (call.endCause === EndCause.UNSPECIFIED) return "";
  const detail =
    call.endCause === EndCause.FAILED
      ? [call.endCode ? String(call.endCode) : "", call.endReason].filter(Boolean).join(" ")
      : call.endCause === EndCause.ERROR
        ? call.endReason
        : "";
  return detail || label(endCauseNames, call.endCause, EndCause[call.endCause]);
}
