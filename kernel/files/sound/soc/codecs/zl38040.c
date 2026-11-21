// SPDX-License-Identifier: GPL-2.0-only
//
// Codec driver for the Microsemi ZL38040 ("Timberwolf") voice processor as
// used in the Digium D80 desk phone.
//
// The host bus interface and the firmware boot sequence are the same as in
// zl38060.c (Copyright(c) 2020 Sven Van Asbroeck) and were copied from it.
// Everything else follows the phone's stock 3.0.35 kernel driver:
//
// - the CPU is connected to TDM-B and is bit and frame clock provider (I2S,
//   two 32-bit slots, voice in the left channel);
// - a Bluetooth module is connected to TDM-A, where the chip provides the
//   clocks;
// - the chip runs its echo canceller with one of four call paths (speaker,
//   handset, headset, Bluetooth), each of which has a cross-point table, an
//   equalizer table, and an echo-canceller table;
// - the receive (ROUT) gain is ramped down before a path change and ramped
//   back up afterwards.
//
// The chip must be strapped for "host boot": the firmware is provided by this
// driver.

#include <linux/delay.h>
#include <linux/gpio/consumer.h>
#include <linux/ihex.h>
#include <linux/minmax.h>
#include <linux/mod_devicetable.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/regmap.h>
#include <linux/spi/spi.h>
#include <linux/workqueue.h>

#include <sound/core.h>
#include <sound/pcm.h>
#include <sound/pcm_params.h>
#include <sound/soc.h>

#include "zl38040-tables.h"

#define DRV_NAME		"zl38040"
#define ZL38040_FIRMWARE	"zl38040.fw"

/* What the stock driver's DAI advertises. */
#define ZL38040_RATES		(SNDRV_PCM_RATE_8000 | SNDRV_PCM_RATE_16000)
#define ZL38040_FORMATS		SNDRV_PCM_FMTBIT_S16_LE

#define HBI_FIRMWARE_PAGE	0xFF
#define ZL38040_MAX_RAW_XFER	0x100

/* Page 0: boot and status registers. */
#define REG_SEMA_FLAGS		0x0006
#define SEMA_FLAGS_BOOT_CMD	BIT(0)
#define SEMA_FLAGS_APP_REBOOT	BIT(1)
#define REG_PG255_BASE_HI	0x000C
#define REG_PG255_OFFS(addr)	((HBI_FIRMWARE_PAGE << 8) | ((addr) & 0xFF))
#define REG_CLK_STATUS		0x0014
#define CLK_STATUS_HWRST	BIT(0)
#define REG_HW_REV		0x0020
#define REG_FW_PROD		0x0022
#define REG_FW_REV		0x0024
#define REG_CMD			0x0032
#define REG_PARAM_RESULT	0x0034
#define PARAM_RESULT_READY	0xD3D3
#define REG_FWR_EXEC		0x012C

#define BOOTCMD_FW_GO		0x0008
#define BOOTCMD_LOAD_COMPLETE	0x000D

/* Audio path configuration, 0x202..0x2be. */
#define REG_XP_FIRST		0x0202
#define REG_TDMA_CFG_CLK	0x0262
#define REG_TDMB_CFG_CLK	0x027A
#define CFG_CLK_MASTER		BIT(15)
#define CFG_CLK_PCLK(bits)	(((bits) - 1) << 4)
#define CFG_CLK_FSRATE_MASK	0x7
#define CFG_CLK_FSRATE_8KHZ	0x1
#define CFG_CLK_FSRATE_16KHZ	0x2
/* The stock driver writes 5 here after every cross-point table. */
#define REG_XP_FINAL		0x02B0
#define XP_FINAL_VALUE		0x0005

/* Echo canceller. */
#define REG_AEC_CTL0		0x0300
#define AEC_CTL0_MUTE_ROUT	BIT(7)
#define REG_AEC_CTL1		0x0302
#define REG_ROUT_GAIN		0x030A
#define ROUT_GAIN_MASK		0x7F
/* Set by the stock driver whenever the headset path is active. */
#define ROUT_GAIN_HEADSET	BIT(7)
#define REG_EC_FIRST		0x030C
/* Extra speaker gain at the highest volume settings; 0x64 is the default. */
#define REG_SPKR_BOOST		0x0310
#define SPKR_BOOST_DEFAULT	0x64

/* Equalizer. */
#define REG_EQ_FIRST		0x0500

/* The stock driver's gain ramp. */
#define ZL38040_RAMP_MS		10
#define ZL38040_RAMP_DOWN_STEP	3
#define ZL38040_RAMP_UP_STEP	2
#define ZL38040_VOLUME_MAX	10

enum zl38040_path {
	ZL38040_PATH_SPEAKER,
	ZL38040_PATH_HANDSET,
	ZL38040_PATH_HEADSET,
	ZL38040_PATH_BLUETOOTH,
	ZL38040_NUM_PATHS,
};

struct zl38040_path_cfg {
	const u16 *xp;		/* 0x202..0x2be */
	const u16 *eq;		/* 0x500..0x6aa */
	const u16 *ec;		/* 0x30c..0x3d2 */
	u16 aec_ctl0;
	u16 aec_ctl1;
	u8 rout_base;		/* ROUT gain at volume 5 */
};

static const struct zl38040_path_cfg zl38040_paths[ZL38040_NUM_PATHS] = {
	[ZL38040_PATH_SPEAKER] = {
		.xp = zl38040_xp_speaker,
		.eq = zl38040_eq_speaker,
		.ec = zl38040_ec_speaker,
		.aec_ctl0 = 0x4800,
		.aec_ctl1 = 0x0800,
		.rout_base = 0x20,
	},
	[ZL38040_PATH_HANDSET] = {
		.xp = zl38040_xp_handset,
		.eq = zl38040_eq_handset,
		.ec = zl38040_ec_handset,
		.aec_ctl0 = 0x4810,
		.aec_ctl1 = 0x0000,
		.rout_base = 0x20,
	},
	[ZL38040_PATH_HEADSET] = {
		.xp = zl38040_xp_headset,
		.eq = zl38040_eq_headset,
		.ec = zl38040_ec_headset,
		.aec_ctl0 = 0x2830,
		.aec_ctl1 = 0x4020,
		.rout_base = 0x20,
	},
	[ZL38040_PATH_BLUETOOTH] = {
		.xp = zl38040_xp_bluetooth,
		.eq = zl38040_eq_bluetooth,
		.ec = zl38040_ec_bluetooth,
		.aec_ctl0 = 0x2830,
		.aec_ctl1 = 0x4020,
		.rout_base = 0x48,
	},
};

struct zl38040_priv {
	struct device *dev;
	struct regmap *regmap;
	struct gpio_desc *handset_gpio;
	struct gpio_desc *amp_gpio;
	struct work_struct work;

	/* Protects everything below and multi-register sequences. */
	struct mutex lock;
	unsigned int path;		/* requested path */
	unsigned int cur_path;		/* path programmed into the chip */
	unsigned int volume;		/* 0 (mute) .. ZL38040_VOLUME_MAX */
	unsigned int bt_wideband;	/* TDM-A at 16 kHz instead of 8 kHz */
	unsigned int fsrate;		/* TDM-B CFG_CLK_FSRATE_* */
	int rout_gain;			/* gain field of REG_ROUT_GAIN */
	bool reload;			/* reprogram the current path */
	bool stream_muted;
	bool failed;
	/*
	 * As a clock consumer the chip only starts passing audio on TDM-B after
	 * a soft reset taken while the bit clock is running (seen on hardware:
	 * otherwise both directions stay silent).  "synced" records that such a
	 * reset happened since the last one taken without clocks.
	 */
	atomic_t running;		/* running streams, set from triggers */
	unsigned int streams;		/* copy of "running" taken under the lock */
	bool synced;
};

static int zl38040_fw_issue_command(struct regmap *regmap, u16 cmd)
{
	unsigned int val;
	int err;

	err = regmap_read_poll_timeout(regmap, REG_SEMA_FLAGS, val,
				       !(val & SEMA_FLAGS_BOOT_CMD), 10000,
				       10000 * 100);
	if (err)
		return err;
	err = regmap_write(regmap, REG_CMD, cmd);
	if (err)
		return err;
	err = regmap_update_bits(regmap, REG_SEMA_FLAGS, SEMA_FLAGS_BOOT_CMD,
				 SEMA_FLAGS_BOOT_CMD);
	if (err)
		return err;

	return regmap_read_poll_timeout(regmap, REG_CMD, val, !val, 10000,
					10000 * 100);
}

static int zl38040_fw_go(struct device *dev, struct regmap *regmap)
{
	unsigned int val;
	int err;

	err = zl38040_fw_issue_command(regmap, BOOTCMD_LOAD_COMPLETE);
	if (err)
		return err;

	/* The stock driver refuses to start an image the boot ROM rejected. */
	err = regmap_read(regmap, REG_PARAM_RESULT, &val);
	if (err)
		return err;
	if (val) {
		dev_err(dev, "firmware load failed, result 0x%04x\n", val);
		return -EIO;
	}

	return zl38040_fw_issue_command(regmap, BOOTCMD_FW_GO);
}

static int zl38040_fw_enter_boot_mode(struct regmap *regmap)
{
	unsigned int val;
	int err;

	err = regmap_update_bits(regmap, REG_CLK_STATUS, CLK_STATUS_HWRST,
				 CLK_STATUS_HWRST);
	if (err)
		return err;

	return regmap_read_poll_timeout(regmap, REG_PARAM_RESULT, val,
					val == PARAM_RESULT_READY, 1000, 50000);
}

static int zl38040_fw_send_data(struct regmap *regmap, u32 addr,
				const void *data, u16 len)
{
	__be32 addr_base = cpu_to_be32(addr & ~0xFF);
	int err;

	err = regmap_raw_write(regmap, REG_PG255_BASE_HI, &addr_base,
			       sizeof(addr_base));
	if (err)
		return err;
	return regmap_raw_write(regmap, REG_PG255_OFFS(addr), data, len);
}

static int zl38040_fw_send_xaddr(struct regmap *regmap, const void *data)
{
	/* execution address from ihex: 32-bit little endian.
	 * device register expects 32-bit big endian.
	 */
	u32 addr = le32_to_cpup(data);
	__be32 baddr = cpu_to_be32(addr);

	return regmap_raw_write(regmap, REG_FWR_EXEC, &baddr, sizeof(baddr));
}

static int zl38040_load_firmware(struct device *dev, struct regmap *regmap)
{
	const struct ihex_binrec *rec;
	const struct firmware *fw __free(firmware) = NULL;
	u32 addr;
	u16 len;
	int err;

	/* The firmware is Microsemi's S-record image converted to ihex and
	 * then to binary records with the kernel's ihex2fw tool (-j, so that
	 * the execution address record is kept), as for zl38060.fw.
	 */
	err = request_ihex_firmware(&fw, ZL38040_FIRMWARE, dev);
	if (err)
		return err;
	err = zl38040_fw_enter_boot_mode(regmap);
	if (err)
		return err;
	rec = (const struct ihex_binrec *)fw->data;
	while (rec) {
		addr = be32_to_cpu(rec->addr);
		len = be16_to_cpu(rec->len);
		if (addr) {
			/* regular data ihex record */
			err = zl38040_fw_send_data(regmap, addr, rec->data,
						   len);
		} else if (len == 4) {
			/* execution address ihex record */
			err = zl38040_fw_send_xaddr(regmap, rec->data);
		} else {
			err = -EINVAL;
		}
		if (err)
			return err;
		rec = ihex_next_binrec(rec);
	}

	return zl38040_fw_go(dev, regmap);
}

/* The chip requires a software reset to apply audio register changes. */
static int zl38040_software_reset(struct regmap *regmap)
{
	unsigned int val;
	int err;

	err = regmap_update_bits(regmap, REG_SEMA_FLAGS, SEMA_FLAGS_APP_REBOOT,
				 SEMA_FLAGS_APP_REBOOT);
	if (err)
		return err;

	/* wait for host bus interface to settle, as zl38060.c does */
	msleep(50);

	return regmap_read_poll_timeout(regmap, REG_SEMA_FLAGS, val,
					!(val & SEMA_FLAGS_APP_REBOOT), 10000,
					10000 * 100);
}

static int zl38040_write_table(struct regmap *regmap, unsigned int first,
			       const u16 *table, size_t count)
{
	size_t i;
	int err;

	for (i = 0; i < count; i++) {
		err = regmap_write(regmap, first + 2 * i, table[i]);
		if (err)
			return err;
	}

	return 0;
}

static bool zl38040_muted(struct zl38040_priv *priv)
{
	return priv->stream_muted || !priv->volume;
}

static int zl38040_write_aec_ctl0(struct zl38040_priv *priv, bool mute)
{
	u16 val = zl38040_paths[priv->cur_path].aec_ctl0;

	if (mute)
		val |= AEC_CTL0_MUTE_ROUT;

	return regmap_write(priv->regmap, REG_AEC_CTL0, val);
}

static int zl38040_write_rout_gain(struct zl38040_priv *priv)
{
	u16 val = priv->rout_gain & ROUT_GAIN_MASK;

	if (priv->cur_path == ZL38040_PATH_HEADSET)
		val |= ROUT_GAIN_HEADSET;

	return regmap_write(priv->regmap, REG_ROUT_GAIN, val);
}

/* ROUT gain for the requested volume: eight steps per volume step. */
static int zl38040_rout_target(struct zl38040_priv *priv)
{
	int base = zl38040_paths[priv->cur_path].rout_base;

	return clamp(base + ((int)priv->volume - 5) * 8, 0, ROUT_GAIN_MASK);
}

static u16 zl38040_spkr_boost(struct zl38040_priv *priv)
{
	if (priv->cur_path != ZL38040_PATH_SPEAKER)
		return SPKR_BOOST_DEFAULT;

	switch (priv->volume) {
	case 8:
		return 0x78;
	case 9:
		return 0x96;
	case 10:
		return 0xC8;
	default:
		return SPKR_BOOST_DEFAULT;
	}
}

/* Program a call path.  The caller has ramped the ROUT gain down to 0. */
static int zl38040_apply_path(struct zl38040_priv *priv, unsigned int path)
{
	const struct zl38040_path_cfg *cfg = &zl38040_paths[path];
	struct regmap *regmap = priv->regmap;
	int err;

	gpiod_set_value_cansleep(priv->amp_gpio, 0);

	/* One DAC and microphone input serve both the handset and headset. */
	if (path == ZL38040_PATH_HANDSET)
		gpiod_set_value_cansleep(priv->handset_gpio, 1);
	else if (path == ZL38040_PATH_HEADSET)
		gpiod_set_value_cansleep(priv->handset_gpio, 0);

	priv->cur_path = path;

	err = zl38040_write_table(regmap, REG_XP_FIRST, cfg->xp,
				  ARRAY_SIZE(zl38040_xp_speaker));
	if (err)
		return err;
	err = regmap_write(regmap, REG_XP_FINAL, XP_FINAL_VALUE);
	if (err)
		return err;

	if (path == ZL38040_PATH_BLUETOOTH) {
		err = regmap_write(regmap, REG_TDMA_CFG_CLK,
				   CFG_CLK_MASTER | CFG_CLK_PCLK(32) |
				   (priv->bt_wideband ? CFG_CLK_FSRATE_16KHZ :
							CFG_CLK_FSRATE_8KHZ));
		if (err)
			return err;
	}

	/* The tables set TDM-B up for 16 kHz. */
	err = regmap_update_bits(regmap, REG_TDMB_CFG_CLK, CFG_CLK_FSRATE_MASK,
				 priv->fsrate);
	if (err)
		return err;

	err = zl38040_write_aec_ctl0(priv, true);
	if (err)
		return err;
	err = regmap_write(regmap, REG_AEC_CTL1, cfg->aec_ctl1);
	if (err)
		return err;
	err = zl38040_write_table(regmap, REG_EQ_FIRST, cfg->eq,
				  ARRAY_SIZE(zl38040_eq_speaker));
	if (err)
		return err;
	err = zl38040_write_table(regmap, REG_EC_FIRST, cfg->ec,
				  ARRAY_SIZE(zl38040_ec_speaker));
	if (err)
		return err;

	err = zl38040_software_reset(regmap);
	if (err)
		return err;
	priv->synced = priv->streams > 0;

	if (path == ZL38040_PATH_SPEAKER)
		gpiod_set_value_cansleep(priv->amp_gpio, 1);

	return 0;
}

/*
 * One step of the stock driver's gain ramp, which runs every 10 ms: ramp down
 * and switch if another path was requested, otherwise ramp to the requested
 * volume.  Returns a positive value when there is nothing left to do.
 */
static int zl38040_step(struct zl38040_priv *priv)
{
	int target, err;

	if (!priv->synced && priv->streams) {
		/* Let the bit clock settle, then lock the chip onto it. */
		msleep(20);
		err = zl38040_software_reset(priv->regmap);
		if (err)
			return err;
		priv->synced = true;
		return 0;
	}

	if (priv->path != priv->cur_path || priv->reload) {
		if (priv->rout_gain > 0) {
			priv->rout_gain = max(priv->rout_gain -
					      ZL38040_RAMP_DOWN_STEP, 0);
			return zl38040_write_rout_gain(priv);
		}

		priv->reload = false;
		return zl38040_apply_path(priv, priv->path);
	}

	err = zl38040_write_aec_ctl0(priv, zl38040_muted(priv));
	if (err)
		return err;

	target = zl38040_rout_target(priv);
	if (priv->rout_gain == target) {
		err = regmap_write(priv->regmap, REG_SPKR_BOOST,
				   zl38040_spkr_boost(priv));
		return err ? err : 1;
	}

	priv->rout_gain += clamp(target - priv->rout_gain,
				 -ZL38040_RAMP_UP_STEP, ZL38040_RAMP_UP_STEP);

	return zl38040_write_rout_gain(priv);
}

static void zl38040_work(struct work_struct *work)
{
	struct zl38040_priv *priv = container_of(work, struct zl38040_priv,
						 work);
	int ret;

	for (;;) {
		mutex_lock(&priv->lock);
		priv->streams = atomic_read(&priv->running);
		ret = zl38040_step(priv);
		if (ret < 0) {
			/* Do not retry forever; a later request tries again. */
			priv->failed = true;
			priv->cur_path = priv->path;
			priv->reload = false;
		}
		mutex_unlock(&priv->lock);

		if (ret < 0)
			dev_err(priv->dev, "path/gain update failed: %d\n",
				ret);
		if (ret)
			break;

		msleep(ZL38040_RAMP_MS);
	}
}

static int zl38040_path_get(struct snd_kcontrol *kcontrol,
			    struct snd_ctl_elem_value *ucontrol)
{
	struct snd_soc_component *component = snd_kcontrol_chip(kcontrol);
	struct zl38040_priv *priv = snd_soc_component_get_drvdata(component);

	mutex_lock(&priv->lock);
	ucontrol->value.enumerated.item[0] = priv->path;
	mutex_unlock(&priv->lock);

	return 0;
}

static int zl38040_path_put(struct snd_kcontrol *kcontrol,
			    struct snd_ctl_elem_value *ucontrol)
{
	struct snd_soc_component *component = snd_kcontrol_chip(kcontrol);
	struct zl38040_priv *priv = snd_soc_component_get_drvdata(component);
	unsigned int path = ucontrol->value.enumerated.item[0];
	bool changed;

	if (path >= ZL38040_NUM_PATHS)
		return -EINVAL;

	mutex_lock(&priv->lock);
	changed = path != priv->path;
	priv->path = path;
	/* Selecting the path again retries after a failed update. */
	if (priv->failed) {
		priv->failed = false;
		priv->reload = true;
	}
	mutex_unlock(&priv->lock);

	schedule_work(&priv->work);

	return changed;
}

static int zl38040_volume_get(struct snd_kcontrol *kcontrol,
			      struct snd_ctl_elem_value *ucontrol)
{
	struct snd_soc_component *component = snd_kcontrol_chip(kcontrol);
	struct zl38040_priv *priv = snd_soc_component_get_drvdata(component);

	mutex_lock(&priv->lock);
	ucontrol->value.integer.value[0] = priv->volume;
	mutex_unlock(&priv->lock);

	return 0;
}

static int zl38040_volume_put(struct snd_kcontrol *kcontrol,
			      struct snd_ctl_elem_value *ucontrol)
{
	struct snd_soc_component *component = snd_kcontrol_chip(kcontrol);
	struct zl38040_priv *priv = snd_soc_component_get_drvdata(component);
	long volume = ucontrol->value.integer.value[0];
	bool changed;

	if (volume < 0 || volume > ZL38040_VOLUME_MAX)
		return -EINVAL;

	mutex_lock(&priv->lock);
	changed = volume != priv->volume;
	priv->volume = volume;
	mutex_unlock(&priv->lock);

	if (changed)
		schedule_work(&priv->work);

	return changed;
}

static int zl38040_bt_rate_get(struct snd_kcontrol *kcontrol,
			       struct snd_ctl_elem_value *ucontrol)
{
	struct snd_soc_component *component = snd_kcontrol_chip(kcontrol);
	struct zl38040_priv *priv = snd_soc_component_get_drvdata(component);

	mutex_lock(&priv->lock);
	ucontrol->value.enumerated.item[0] = priv->bt_wideband;
	mutex_unlock(&priv->lock);

	return 0;
}

static int zl38040_bt_rate_put(struct snd_kcontrol *kcontrol,
			       struct snd_ctl_elem_value *ucontrol)
{
	struct snd_soc_component *component = snd_kcontrol_chip(kcontrol);
	struct zl38040_priv *priv = snd_soc_component_get_drvdata(component);
	unsigned int wideband = ucontrol->value.enumerated.item[0];
	bool changed;

	if (wideband > 1)
		return -EINVAL;

	mutex_lock(&priv->lock);
	changed = wideband != priv->bt_wideband;
	priv->bt_wideband = wideband;
	/* The TDM-A clock change needs the software reset of a path load. */
	if (changed && priv->path == ZL38040_PATH_BLUETOOTH)
		priv->reload = true;
	mutex_unlock(&priv->lock);

	if (changed)
		schedule_work(&priv->work);

	return changed;
}

static const char * const zl38040_path_texts[] = {
	[ZL38040_PATH_SPEAKER] = "Speaker",
	[ZL38040_PATH_HANDSET] = "Handset",
	[ZL38040_PATH_HEADSET] = "Headset",
	[ZL38040_PATH_BLUETOOTH] = "Bluetooth",
};

static const char * const zl38040_bt_rate_texts[] = { "8000", "16000" };

static SOC_ENUM_SINGLE_EXT_DECL(zl38040_path_enum, zl38040_path_texts);
static SOC_ENUM_SINGLE_EXT_DECL(zl38040_bt_rate_enum, zl38040_bt_rate_texts);

static const struct snd_kcontrol_new zl38040_controls[] = {
	SOC_ENUM_EXT("Audio Path", zl38040_path_enum,
		     zl38040_path_get, zl38040_path_put),
	SOC_SINGLE_EXT("Master Playback Volume", SND_SOC_NOPM, 0,
		       ZL38040_VOLUME_MAX, 0,
		       zl38040_volume_get, zl38040_volume_put),
	SOC_ENUM_EXT("Bluetooth Sample Rate", zl38040_bt_rate_enum,
		     zl38040_bt_rate_get, zl38040_bt_rate_put),
};

static int zl38040_set_fmt(struct snd_soc_dai *dai, unsigned int fmt)
{
	/* The path tables set TDM-B up as an I2S clock consumer. */
	if ((fmt & SND_SOC_DAIFMT_FORMAT_MASK) != SND_SOC_DAIFMT_I2S)
		return -EINVAL;
	if ((fmt & SND_SOC_DAIFMT_INV_MASK) != SND_SOC_DAIFMT_NB_NF)
		return -EINVAL;
	if ((fmt & SND_SOC_DAIFMT_CLOCK_PROVIDER_MASK) !=
	    SND_SOC_DAIFMT_CBC_CFC)
		return -EINVAL;

	return 0;
}

static int zl38040_hw_params(struct snd_pcm_substream *substream,
			     struct snd_pcm_hw_params *params,
			     struct snd_soc_dai *dai)
{
	struct zl38040_priv *priv = snd_soc_dai_get_drvdata(dai);
	unsigned int fsrate;
	bool changed;
	int err;

	switch (params_rate(params)) {
	case 8000:
		fsrate = CFG_CLK_FSRATE_8KHZ;
		break;
	case 16000:
		fsrate = CFG_CLK_FSRATE_16KHZ;
		break;
	default:
		return -EINVAL;
	}

	/* Rates are symmetric, so a second stream changes nothing and does
	 * not reset the chip under the first one.
	 */
	mutex_lock(&priv->lock);
	priv->fsrate = fsrate;
	err = regmap_update_bits_check(priv->regmap, REG_TDMB_CFG_CLK,
				       CFG_CLK_FSRATE_MASK, fsrate, &changed);
	if (!err && changed) {
		err = zl38040_software_reset(priv->regmap);
		priv->synced = false;
	}
	mutex_unlock(&priv->lock);

	return err;
}

/* ROUT carries only what the CPU plays, so mute it while nothing plays. */
static int zl38040_mute_stream(struct snd_soc_dai *dai, int mute, int stream)
{
	struct zl38040_priv *priv = snd_soc_dai_get_drvdata(dai);
	int err;

	mutex_lock(&priv->lock);
	priv->stream_muted = mute;
	err = zl38040_write_aec_ctl0(priv, zl38040_muted(priv));
	mutex_unlock(&priv->lock);

	return err;
}

/*
 * The CPU DAI starts and stops the clocks at trigger time.  Triggers may be
 * atomic, so only count streams here and let the worker do the reset.
 */
static int zl38040_trigger(struct snd_pcm_substream *substream, int cmd,
			   struct snd_soc_dai *dai)
{
	struct zl38040_priv *priv = snd_soc_dai_get_drvdata(dai);

	switch (cmd) {
	case SNDRV_PCM_TRIGGER_START:
	case SNDRV_PCM_TRIGGER_RESUME:
	case SNDRV_PCM_TRIGGER_PAUSE_RELEASE:
		atomic_inc(&priv->running);
		schedule_work(&priv->work);
		break;
	case SNDRV_PCM_TRIGGER_STOP:
	case SNDRV_PCM_TRIGGER_SUSPEND:
	case SNDRV_PCM_TRIGGER_PAUSE_PUSH:
		atomic_dec(&priv->running);
		break;
	}

	return 0;
}

static const struct snd_soc_dai_ops zl38040_dai_ops = {
	.set_fmt = zl38040_set_fmt,
	.hw_params = zl38040_hw_params,
	.trigger = zl38040_trigger,
	.mute_stream = zl38040_mute_stream,
	.no_capture_mute = 1,
};

static struct snd_soc_dai_driver zl38040_dai = {
	.name = "zl38040-tdmb",
	.playback = {
		.stream_name = "Playback",
		.channels_min = 2,
		.channels_max = 2,
		.rates = ZL38040_RATES,
		.formats = ZL38040_FORMATS,
	},
	.capture = {
		.stream_name = "Capture",
		.channels_min = 2,
		.channels_max = 2,
		.rates = ZL38040_RATES,
		.formats = ZL38040_FORMATS,
	},
	.ops = &zl38040_dai_ops,
	.symmetric_rate = 1,
	.symmetric_sample_bits = 1,
	.symmetric_channels = 1,
};

static const struct snd_soc_dapm_widget zl38040_dapm_widgets[] = {
	SND_SOC_DAPM_OUTPUT("DAC1"),
	SND_SOC_DAPM_OUTPUT("DAC2"),
	SND_SOC_DAPM_OUTPUT("TDMA OUT"),

	SND_SOC_DAPM_INPUT("MIC1"),
	SND_SOC_DAPM_INPUT("MIC3"),
	SND_SOC_DAPM_INPUT("TDMA IN"),
};

static const struct snd_soc_dapm_route zl38040_dapm_routes[] = {
	{ "DAC1", NULL, "Playback" },
	{ "DAC2", NULL, "Playback" },
	{ "TDMA OUT", NULL, "Playback" },

	{ "Capture", NULL, "MIC1" },
	{ "Capture", NULL, "MIC3" },
	{ "Capture", NULL, "TDMA IN" },
};

static const struct snd_soc_component_driver zl38040_component_dev = {
	.controls		= zl38040_controls,
	.num_controls		= ARRAY_SIZE(zl38040_controls),
	.dapm_widgets		= zl38040_dapm_widgets,
	.num_dapm_widgets	= ARRAY_SIZE(zl38040_dapm_widgets),
	.dapm_routes		= zl38040_dapm_routes,
	.num_dapm_routes	= ARRAY_SIZE(zl38040_dapm_routes),
	.endianness		= 1,
};

static void zl38040_log_revision(struct device *dev, struct regmap *regmap)
{
	unsigned int hwrev, fwprod, fwrev;

	if (regmap_read(regmap, REG_HW_REV, &hwrev) ||
	    regmap_read(regmap, REG_FW_PROD, &fwprod) ||
	    regmap_read(regmap, REG_FW_REV, &fwrev))
		return;

	dev_info(dev, "hw rev 0x%x, fw product code %u, firmware rev %u.%u.%u\n",
		 hwrev & 0x1F, fwprod, (fwrev >> 12) & 0xF, (fwrev >> 8) & 0xF,
		 fwrev & 0xFF);
}

static int zl38040_bus_read(void *context,
			    const void *reg_buf, size_t reg_size,
			    void *val_buf, size_t val_size)
{
	struct spi_device *spi = context;
	const u8 *reg_buf8 = reg_buf;
	size_t len = 0;
	u8 offs, page;
	u8 txbuf[4];

	if (reg_size != 2 || val_size > ZL38040_MAX_RAW_XFER)
		return -EINVAL;

	offs = reg_buf8[1] >> 1;
	page = reg_buf8[0];

	if (page) {
		txbuf[len++] = 0xFE;
		txbuf[len++] = page == HBI_FIRMWARE_PAGE ? 0xFF : page - 1;
		txbuf[len++] = offs;
		txbuf[len++] = val_size / 2 - 1;
	} else {
		txbuf[len++] = offs | 0x80;
		txbuf[len++] = val_size / 2 - 1;
	}

	return spi_write_then_read(spi, txbuf, len, val_buf, val_size);
}

static int zl38040_bus_write(void *context, const void *data, size_t count)
{
	struct spi_device *spi = context;
	u8 buf[4 + ZL38040_MAX_RAW_XFER];
	size_t val_len, len = 0;
	const u8 *data8 = data;
	u8 offs, page;

	if (count > (2 + ZL38040_MAX_RAW_XFER) || count < 4)
		return -EINVAL;
	val_len = count - 2;
	offs = data8[1] >> 1;
	page = data8[0];

	if (page) {
		buf[len++] = 0xFE;
		buf[len++] = page == HBI_FIRMWARE_PAGE ? 0xFF : page - 1;
		buf[len++] = offs;
		buf[len++] = (val_len / 2 - 1) | 0x80;
	} else {
		buf[len++] = offs | 0x80;
		buf[len++] = (val_len / 2 - 1) | 0x80;
	}
	memcpy(buf + len, data8 + 2, val_len);
	len += val_len;

	return spi_write(spi, buf, len);
}

static const struct regmap_bus zl38040_regmap_bus = {
	.read = zl38040_bus_read,
	.write = zl38040_bus_write,
	.max_raw_write = ZL38040_MAX_RAW_XFER,
	.max_raw_read = ZL38040_MAX_RAW_XFER,
};

static const struct regmap_config zl38040_regmap_conf = {
	.reg_bits = 16,
	.val_bits = 16,
	.reg_stride = 2,
	/* Lets the regmap debugfs file dump the chip's registers. */
	.max_register = 0x06FE,
	.use_single_read = true,
	.use_single_write = true,
};

static void zl38040_stop(void *data)
{
	struct zl38040_priv *priv = data;

	cancel_work_sync(&priv->work);
	gpiod_set_value_cansleep(priv->amp_gpio, 0);
}

static int zl38040_spi_probe(struct spi_device *spi)
{
	struct device *dev = &spi->dev;
	struct zl38040_priv *priv;
	struct gpio_desc *reset_gpio;
	int err;

	priv = devm_kzalloc(dev, sizeof(*priv), GFP_KERNEL);
	if (!priv)
		return -ENOMEM;

	priv->dev = dev;
	priv->fsrate = CFG_CLK_FSRATE_16KHZ;
	priv->bt_wideband = 1;
	priv->stream_muted = true;
	INIT_WORK(&priv->work, zl38040_work);
	err = devm_mutex_init(dev, &priv->lock);
	if (err)
		return err;
	dev_set_drvdata(dev, priv);

	/* Keep the speaker amplifier off until the speaker path is loaded. */
	priv->amp_gpio = devm_gpiod_get_optional(dev, "speaker-amp",
						 GPIOD_OUT_LOW);
	if (IS_ERR(priv->amp_gpio))
		return PTR_ERR(priv->amp_gpio);
	priv->handset_gpio = devm_gpiod_get_optional(dev, "handset-select",
						     GPIOD_OUT_HIGH);
	if (IS_ERR(priv->handset_gpio))
		return PTR_ERR(priv->handset_gpio);

	/* get the chip to a known state by putting it in reset */
	reset_gpio = devm_gpiod_get_optional(dev, "reset", GPIOD_OUT_HIGH);
	if (IS_ERR(reset_gpio))
		return PTR_ERR(reset_gpio);
	if (reset_gpio) {
		/* datasheet: need > 10us for a digital + analog reset */
		usleep_range(15, 50);
		/* take the chip out of reset */
		gpiod_set_value_cansleep(reset_gpio, 0);
		/* datasheet: need > 3ms for digital section to become stable */
		usleep_range(3000, 10000);
	}

	priv->regmap = devm_regmap_init(dev, &zl38040_regmap_bus, spi,
					&zl38040_regmap_conf);
	if (IS_ERR(priv->regmap))
		return PTR_ERR(priv->regmap);

	err = zl38040_load_firmware(dev, priv->regmap);
	if (err)
		return dev_err_probe(dev, err, "failed to boot firmware\n");

	zl38040_log_revision(dev, priv->regmap);

	/* The stock driver's defaults, then the speaker path, muted. */
	err = zl38040_write_table(priv->regmap, REG_XP_FIRST,
				  zl38040_default_regs,
				  ARRAY_SIZE(zl38040_default_regs));
	if (err)
		return err;

	err = devm_add_action_or_reset(dev, zl38040_stop, priv);
	if (err)
		return err;

	mutex_lock(&priv->lock);
	err = zl38040_apply_path(priv, ZL38040_PATH_SPEAKER);
	if (!err)
		err = zl38040_write_rout_gain(priv);
	mutex_unlock(&priv->lock);
	if (err)
		return err;

	return devm_snd_soc_register_component(dev, &zl38040_component_dev,
					       &zl38040_dai, 1);
}

static const struct of_device_id zl38040_dt_ids[] = {
	{ .compatible = "mscc,zl38040", },
	{ /* sentinel */ }
};
MODULE_DEVICE_TABLE(of, zl38040_dt_ids);

static const struct spi_device_id zl38040_spi_ids[] = {
	{ "zl38040", 0 },
	{ /* sentinel */ }
};
MODULE_DEVICE_TABLE(spi, zl38040_spi_ids);

static struct spi_driver zl38040_spi_driver = {
	.driver	= {
		.name = DRV_NAME,
		.of_match_table = zl38040_dt_ids,
	},
	.probe = zl38040_spi_probe,
	.id_table = zl38040_spi_ids,
};
module_spi_driver(zl38040_spi_driver);

MODULE_DESCRIPTION("ASoC ZL38040 driver");
MODULE_FIRMWARE(ZL38040_FIRMWARE);
MODULE_LICENSE("GPL v2");
