import { useMutation, useQuery } from "@connectrpc/connect-query";
import { useState } from "react";
import { Alert, Badge, Button, Card, Dpma, Field, Input, KeyValue, PageHeader, Select, Spinner, Toggle } from "../components/ui";
import { ConfigService } from "../gen/opendeskphone/v1/config_pb";
import { Ipv4Source, NetPhase, NetworkService, VlanDiscoveryMode, VlanSource, type NetworkSettings } from "../gen/opendeskphone/v1/network_pb";
import { duration } from "../lib/format";
import { addressSourceNames, label, netPhaseNames, vlanSourceNames } from "../lib/labels";
import { useStatus } from "../lib/status";
import { ErrorNote } from "./common";

export function Network() {
  const { status } = useStatus();
  const q = useQuery(NetworkService.method.getNetwork, {});
  const [draft, setDraft] = useState<NetworkSettings>();
  const [error, setError] = useState<unknown>();
  const [saved, setSaved] = useState(false);
  const save = useMutation(NetworkService.method.setNetworkSettings, {
    onSuccess: () => {
      setDraft(undefined);
      setError(undefined);
      setSaved(true);
      q.refetch();
    },
    onError: setError,
  });
  const renew = useMutation(NetworkService.method.renewDhcp, { onError: setError });
  // Which stock settings DPMA set, by setting ID.
  const stock = useQuery(ConfigService.method.listSettings, {});

  if (q.error) return <ErrorNote error={q.error} />;
  if (!q.data) return <Spinner />;
  const net = status?.network ?? q.data.status!;
  const s = draft ?? q.data.settings!;
  const set = <K extends keyof NetworkSettings>(k: K, v: NetworkSettings[K]) => {
    setSaved(false);
    setDraft({ ...s, [k]: v });
  };
  // The stock setting behind each field of the form.
  const ids = {
    dhcp: "network_enable_dhcp",
    staticIp: "network_static_ip_address",
    subnetMask: "network_subnet_mask",
    gateway: "network_default_gateway",
    primaryDns: "network_primary_dns_server",
    secondaryDns: "network_secondary_dns_server",
    domain: "network_domain_name",
    ipv6Enabled: "network_ipv6_enabled",
    vlanDiscoveryMode: "network_vlan_discovery_mode",
    vlanId: "network_vlan_id",
    pcVlanId: "pc_vlan_id",
    sipDscp: "sip_dscp",
    rtpDscp: "rtp_dscp",
    pcQos: "pc_qos",
    sipQos: "sip_qos",
    rtpQos: "rtp_qos",
    dot1xPassthrough: "8021x_passthrough",
  } as const;
  const addressing = ["dhcp", "staticIp", "subnetMask", "gateway", "primaryDns", "secondaryDns"];
  const setting = (k: keyof typeof ids) => stock.data?.settings.find((x) => x.id === ids[k]);
  // The server's static address stands in the form for the phone's
  // address settings, which a save leaves alone.
  const served = (k: keyof typeof ids) => s.addressingProvisioned && addressing.includes(k);
  const dpma = (k: keyof typeof ids) => served(k) || !!setting(k)?.provisioned;
  const locked = (k: keyof typeof ids) => served(k) || !!setting(k)?.frozen;
  const text = (k: "staticIp" | "subnetMask" | "gateway" | "primaryDns" | "secondaryDns" | "domain", label: string) => (
    <Dpma on={dpma(k)}>
      <Field label={label}>
        <Input value={s[k]} disabled={locked(k)} onChange={(e) => set(k, e.target.value)} />
      </Field>
    </Dpma>
  );
  const num = (k: "sipDscp" | "rtpDscp" | "sipQos" | "rtpQos" | "pcQos", label: string, max: number) => (
    <Dpma on={dpma(k)}>
      <Field label={label}>
        <Input type="number" min={0} max={max} value={s[k]} disabled={locked(k)} onChange={(e) => set(k, Number(e.target.value))} />
      </Field>
    </Dpma>
  );
  const vlan = (k: "vlanId" | "pcVlanId", label: string, disabled = false) => (
    <Dpma on={dpma(k)}>
      <Field label={label}>
        <Input type="number" min={1} max={4094} placeholder="None" disabled={disabled || locked(k)} value={s[k] ?? ""} onChange={(e) => set(k, e.target.value === "" ? undefined : Number(e.target.value))} />
      </Field>
    </Dpma>
  );
  const mode = (dhcp: boolean, name: string) => (
    <label className={locked("dhcp") ? "flex items-center gap-2 text-sm opacity-50" : "flex items-center gap-2 text-sm"}>
      <input type="radio" name="addressing" className="size-4 accent-brand-600" checked={s.dhcp === dhcp} disabled={locked("dhcp")} onChange={() => set("dhcp", dhcp)} />
      <span className="text-slate-700 dark:text-slate-300">{name}</span>
    </label>
  );

  return (
    <>
      <PageHeader title="Network" />
      <div className="space-y-5">
        <ErrorNote error={error} />
        {saved && <Alert tone="green" title="Saved." />}
        <div className="grid gap-5 lg:grid-cols-2">
          <Card
            title="Status"
            actions={
              <>
                <Badge tone={net.phase === NetPhase.UP ? "green" : net.phase === NetPhase.FAILED ? "red" : "amber"}>{label(netPhaseNames, net.phase, NetPhase[net.phase])}</Badge>
                {net.ipv4Source === Ipv4Source.DHCP && (
                  <Button size="sm" onClick={() => renew.mutate({})} busy={renew.isPending}>
                    Renew DHCP
                  </Button>
                )}
              </>
            }
          >
            <KeyValue
              rows={[
                ["IP address", net.ipv4Address && <>{net.ipv4Address}/{net.ipv4PrefixLen}{net.ipv4Source !== Ipv4Source.UNSPECIFIED && <> ({label(addressSourceNames, net.ipv4Source, Ipv4Source[net.ipv4Source])})</>}</>],
                ["Gateway", net.ipv4Gateway],
                ["DHCP server", net.dhcpServer],
                ["Lease time", net.leaseSecs ? duration(net.leaseSecs) : ""],
                ["IPv6", net.ipv6Addresses.map((a) => `${a.address}/${a.prefixLen}`).join(", ")],
                ["DNS", net.dnsServers.map((s) => s.address).join(", ")],
                ["Domain", net.domain],
                ["NTP", `${net.ntpServers.map((s) => s.host).join(", ")}${net.ntpSynced ? " (synced)" : ""}`],
                ["DHCP option 66", net.dhcpTftpServer],
                ["Profile", net.profile],
                ["Details", net.detail],
              ]}
            />
          </Card>
          <Card title="Ports and VLANs">
            <KeyValue
              rows={[
                ["MAC address", net.mac],
                ["LAN port", net.lan?.up ? `${net.lan.speed} Mbps, ${net.lan.fullDuplex ? "full" : "half"} duplex` : "No link"],
                ["PC port", !net.pcPortEnabled ? "Disabled" : net.pc?.up ? `${net.pc.speed} Mbps, ${net.pc.fullDuplex ? "full" : "half"} duplex` : "No link"],
                ["Voice VLAN", net.voiceVlan !== undefined ? <>{net.voiceVlan}, priority {net.vlanPriority}{net.vlanSource !== VlanSource.UNSPECIFIED && <> ({label(vlanSourceNames, net.vlanSource, VlanSource[net.vlanSource])})</>}</> : "None"],
                ["PC port VLAN", net.pcVlan !== undefined ? String(net.pcVlan) : ""],
                ["DSCP", String(net.dscp)],
                ["Switch", net.lldpPresent ? net.lldpSystemName : ""],
                ["Switch port", [net.lldpPortId, net.lldpPortDescription].filter(Boolean).join(" · ")],
                ["Switch chassis ID", net.lldpChassisId],
              ]}
            />
          </Card>
        </div>

        <Card
          title="Settings"
          actions={
            <>
              {draft && <Button onClick={() => setDraft(undefined)}>Discard</Button>}
              <Button variant="primary" disabled={!draft} busy={save.isPending} onClick={() => save.mutate({ settings: { ...s, ntpServer: "", timeZone: "" }, apply: true })}>
                Save
              </Button>
            </>
          }
        >
          <div className="space-y-6">
            <section className="space-y-4">
              <Dpma on={dpma("dhcp")} className="flex w-fit gap-6" >
                {mode(false, "Static")}
                {mode(true, "DHCP")}
              </Dpma>
              <div className="grid gap-4 sm:grid-cols-3">
                {!s.dhcp && (
                  <>
                    {text("staticIp", "IP address")}
                    {text("subnetMask", "Subnet mask")}
                    {text("gateway", "Gateway")}
                    {text("primaryDns", "Primary DNS")}
                    {text("secondaryDns", "Secondary DNS")}
                  </>
                )}
                {text("domain", "Domain")}
              </div>
              <Dpma on={dpma("ipv6Enabled")} className="w-fit">
                <Toggle label="IPv6" checked={s.ipv6Enabled} disabled={locked("ipv6Enabled")} onChange={(v) => set("ipv6Enabled", v)} />
              </Dpma>
            </section>
            <section className="grid gap-4 border-t border-slate-100 pt-5 dark:border-slate-800 sm:grid-cols-3">
              <Dpma on={dpma("vlanDiscoveryMode")}>
                <Field label="VLAN discovery">
                  <Select value={s.vlanDiscoveryMode} disabled={locked("vlanDiscoveryMode")} onChange={(e) => set("vlanDiscoveryMode", Number(e.target.value) as VlanDiscoveryMode)}>
                    <option value={VlanDiscoveryMode.LLDP}>LLDP-MED</option>
                    <option value={VlanDiscoveryMode.MANUAL}>Manual</option>
                    <option value={VlanDiscoveryMode.NONE}>None</option>
                  </Select>
                </Field>
              </Dpma>
              {vlan("vlanId", "Voice VLAN ID", s.vlanDiscoveryMode !== VlanDiscoveryMode.MANUAL)}
              {vlan("pcVlanId", "PC port VLAN ID")}
              {num("sipDscp", "SIP DSCP", 63)}
              {num("rtpDscp", "RTP DSCP", 63)}
              {num("pcQos", "PC port 802.1p priority", 7)}
              {num("sipQos", "SIP 802.1p priority", 7)}
              {num("rtpQos", "RTP 802.1p priority", 7)}
              <Dpma on={dpma("dot1xPassthrough")} className="flex items-end pb-2">
                <Toggle label="802.1X passthrough" checked={s.dot1xPassthrough} disabled={locked("dot1xPassthrough")} onChange={(v) => set("dot1xPassthrough", v)} />
              </Dpma>
            </section>
          </div>
        </Card>
      </div>
    </>
  );
}
