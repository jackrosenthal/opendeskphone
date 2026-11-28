// SPDX-License-Identifier: GPL-2.0-only
/*
 * GPIO bank driver for Broadcom BCMRING SoCs (BCM11107 family: BCM11170,
 * BCM11109, BCM11189, BCM11188, ...).
 *
 * New code written from behavior recovered from the vendor kernels of the
 * Digium/Sangoma D40/D45/D50/D70 (2.6.27.18) and D60/D62/D65 (3.4.74)
 * (functions gpio_bcmring_*, gpio_irq_*, gpio_isr_handler0/1,
 * GpioHwReg_SetIrqType).  No vendor or old-mainline source was copied; block
 * addresses and IRQ numbers agree with the old mainline
 * arch/arm/mach-bcmring/include/mach/csp/mm_addr.h and mach/irqs.h (v3.4).
 *
 * The SoC has two identical banks:
 *   GPIO0 @ 0x80004000, lines  0..31, IRQ_GPIO0 (INTC1 bit 0)
 *   GPIO1 @ 0x80004800, lines 32..60, IRQ_GPIO1 (INTC1 bit 1), 29 lines
 * Each bank is one device node / one gpio_chip.
 *
 * The interrupt registers at 0x404..0x41c behave like an ARM PL061
 * (IS/IBE/IEV/IE/RIS/MIS/IC), but the block is not a PL061: it is 32 lines
 * wide and the data register is a plain read/write register at offset 0
 * with no address-bit masking, so gpio-pl061 cannot drive it.
 */

#include <linux/bitops.h>
#include <linux/gpio/driver.h>
#include <linux/interrupt.h>
#include <linux/io.h>
#include <linux/irq.h>
#include <linux/irqchip/chained_irq.h>
#include <linux/mod_devicetable.h>
#include <linux/module.h>
#include <linux/platform_device.h>
#include <linux/property.h>
#include <linux/seq_file.h>
#include <linux/spinlock.h>

/* Register map; all verified from the vendor binaries except RIS. */
#define BCMRING_GPIO_DATA	0x000	/* R: pin level, W: output latch */
#define BCMRING_GPIO_DIR	0x400	/* 1 = output */
#define BCMRING_GPIO_IS		0x404	/* 1 = level sensitive, 0 = edge */
#define BCMRING_GPIO_IBE	0x408	/* 1 = both edges */
#define BCMRING_GPIO_IEV	0x40c	/* 1 = rising edge / high level */
#define BCMRING_GPIO_IE		0x410	/* 1 = interrupt enabled */
#define BCMRING_GPIO_RIS	0x414	/* raw status; INFERRED (PL061 analogy), unused */
#define BCMRING_GPIO_MIS	0x418	/* masked status */
#define BCMRING_GPIO_IC		0x41c	/* write 1 to clear */

struct bcmring_gpio {
	void __iomem *base;
	raw_spinlock_t lock;
	struct gpio_chip gc;
};

static void bcmring_gpio_rmw(struct bcmring_gpio *bg, unsigned int reg,
			     u32 clear, u32 set)
{
	u32 val;

	val = readl(bg->base + reg);
	val &= ~clear;
	val |= set;
	writel(val, bg->base + reg);
}

static int bcmring_gpio_get_direction(struct gpio_chip *gc, unsigned int offset)
{
	struct bcmring_gpio *bg = gpiochip_get_data(gc);

	if (readl(bg->base + BCMRING_GPIO_DIR) & BIT(offset))
		return GPIO_LINE_DIRECTION_OUT;

	return GPIO_LINE_DIRECTION_IN;
}

static int bcmring_gpio_direction_input(struct gpio_chip *gc,
					unsigned int offset)
{
	struct bcmring_gpio *bg = gpiochip_get_data(gc);
	unsigned long flags;

	raw_spin_lock_irqsave(&bg->lock, flags);
	bcmring_gpio_rmw(bg, BCMRING_GPIO_DIR, BIT(offset), 0);
	raw_spin_unlock_irqrestore(&bg->lock, flags);

	return 0;
}

static int bcmring_gpio_direction_output(struct gpio_chip *gc,
					 unsigned int offset, int value)
{
	struct bcmring_gpio *bg = gpiochip_get_data(gc);
	unsigned long flags;

	raw_spin_lock_irqsave(&bg->lock, flags);
	/*
	 * The data latch ignores writes while the line is an input (seen on a
	 * D65), so the level can only be written after the direction.  Until
	 * then the line drives whatever the latch last held.
	 */
	bcmring_gpio_rmw(bg, BCMRING_GPIO_DIR, 0, BIT(offset));
	bcmring_gpio_rmw(bg, BCMRING_GPIO_DATA, BIT(offset),
			 value ? BIT(offset) : 0);
	raw_spin_unlock_irqrestore(&bg->lock, flags);

	return 0;
}

static int bcmring_gpio_get(struct gpio_chip *gc, unsigned int offset)
{
	struct bcmring_gpio *bg = gpiochip_get_data(gc);

	return !!(readl(bg->base + BCMRING_GPIO_DATA) & BIT(offset));
}

static int bcmring_gpio_set(struct gpio_chip *gc, unsigned int offset,
			    int value)
{
	struct bcmring_gpio *bg = gpiochip_get_data(gc);
	unsigned long flags;

	raw_spin_lock_irqsave(&bg->lock, flags);
	bcmring_gpio_rmw(bg, BCMRING_GPIO_DATA, BIT(offset),
			 value ? BIT(offset) : 0);
	raw_spin_unlock_irqrestore(&bg->lock, flags);

	return 0;
}

static void bcmring_gpio_irq_ack(struct irq_data *d)
{
	struct gpio_chip *gc = irq_data_get_irq_chip_data(d);
	struct bcmring_gpio *bg = gpiochip_get_data(gc);

	/*
	 * The vendor code does a read-modify-write OR here, which only makes
	 * sense if IC reads back as zero, i.e., it is write-one-to-clear.
	 */
	writel(BIT(irqd_to_hwirq(d)), bg->base + BCMRING_GPIO_IC);
}

static void bcmring_gpio_irq_mask(struct irq_data *d)
{
	struct gpio_chip *gc = irq_data_get_irq_chip_data(d);
	struct bcmring_gpio *bg = gpiochip_get_data(gc);
	irq_hw_number_t hwirq = irqd_to_hwirq(d);
	unsigned long flags;

	raw_spin_lock_irqsave(&bg->lock, flags);
	bcmring_gpio_rmw(bg, BCMRING_GPIO_IE, BIT(hwirq), 0);
	raw_spin_unlock_irqrestore(&bg->lock, flags);

	gpiochip_disable_irq(gc, hwirq);
}

static void bcmring_gpio_irq_unmask(struct irq_data *d)
{
	struct gpio_chip *gc = irq_data_get_irq_chip_data(d);
	struct bcmring_gpio *bg = gpiochip_get_data(gc);
	irq_hw_number_t hwirq = irqd_to_hwirq(d);
	unsigned long flags;

	gpiochip_enable_irq(gc, hwirq);

	raw_spin_lock_irqsave(&bg->lock, flags);
	bcmring_gpio_rmw(bg, BCMRING_GPIO_IE, 0, BIT(hwirq));
	raw_spin_unlock_irqrestore(&bg->lock, flags);
}

static int bcmring_gpio_irq_set_type(struct irq_data *d, unsigned int type)
{
	struct gpio_chip *gc = irq_data_get_irq_chip_data(d);
	struct bcmring_gpio *bg = gpiochip_get_data(gc);
	u32 bit = BIT(irqd_to_hwirq(d));
	u32 is = 0, ibe = 0, iev = 0;
	unsigned long flags;
	u32 ie;

	/* Same encodings as GpioHwReg_SetIrqType() cases 0..4. */
	switch (type & IRQ_TYPE_SENSE_MASK) {
	case IRQ_TYPE_LEVEL_HIGH:
		is = bit;
		iev = bit;
		break;
	case IRQ_TYPE_LEVEL_LOW:
		is = bit;
		break;
	case IRQ_TYPE_EDGE_RISING:
		iev = bit;
		break;
	case IRQ_TYPE_EDGE_FALLING:
		break;
	case IRQ_TYPE_EDGE_BOTH:
		ibe = bit;
		break;
	default:
		return -EINVAL;
	}

	raw_spin_lock_irqsave(&bg->lock, flags);
	/* Like the vendor code: mask while reprogramming, drop stale edges. */
	ie = readl(bg->base + BCMRING_GPIO_IE);
	writel(ie & ~bit, bg->base + BCMRING_GPIO_IE);
	bcmring_gpio_rmw(bg, BCMRING_GPIO_IS, bit, is);
	bcmring_gpio_rmw(bg, BCMRING_GPIO_IBE, bit, ibe);
	bcmring_gpio_rmw(bg, BCMRING_GPIO_IEV, bit, iev);
	writel(bit, bg->base + BCMRING_GPIO_IC);
	writel(ie, bg->base + BCMRING_GPIO_IE);
	raw_spin_unlock_irqrestore(&bg->lock, flags);

	/*
	 * The vendor kernels use handle_edge_irq for every line.  Picking the
	 * flow handler by trigger type is the conventional arrangement.
	 */
	if (type & IRQ_TYPE_LEVEL_MASK)
		irq_set_handler_locked(d, handle_level_irq);
	else
		irq_set_handler_locked(d, handle_edge_irq);

	return 0;
}

static void bcmring_gpio_irq_print_chip(struct irq_data *d, struct seq_file *p)
{
	struct gpio_chip *gc = irq_data_get_irq_chip_data(d);

	seq_puts(p, dev_name(gc->parent));
}

static const struct irq_chip bcmring_gpio_irq_chip = {
	.irq_ack		= bcmring_gpio_irq_ack,
	.irq_mask		= bcmring_gpio_irq_mask,
	.irq_unmask		= bcmring_gpio_irq_unmask,
	.irq_set_type		= bcmring_gpio_irq_set_type,
	.irq_print_chip		= bcmring_gpio_irq_print_chip,
	.flags			= IRQCHIP_IMMUTABLE,
	GPIOCHIP_IRQ_RESOURCE_HELPERS,
};

static void bcmring_gpio_irq_handler(struct irq_desc *desc)
{
	struct gpio_chip *gc = irq_desc_get_handler_data(desc);
	struct bcmring_gpio *bg = gpiochip_get_data(gc);
	struct irq_chip *chip = irq_desc_get_chip(desc);
	unsigned long pending;
	unsigned int offset;

	chained_irq_enter(chip, desc);

	pending = readl(bg->base + BCMRING_GPIO_MIS);
	for_each_set_bit(offset, &pending, gc->ngpio)
		generic_handle_domain_irq(gc->irq.domain, offset);

	chained_irq_exit(chip, desc);
}

static int bcmring_gpio_probe(struct platform_device *pdev)
{
	struct device *dev = &pdev->dev;
	struct gpio_irq_chip *girq;
	struct bcmring_gpio *bg;
	u32 ngpios = 32;
	int irq;

	bg = devm_kzalloc(dev, sizeof(*bg), GFP_KERNEL);
	if (!bg)
		return -ENOMEM;

	bg->base = devm_platform_ioremap_resource(pdev, 0);
	if (IS_ERR(bg->base))
		return PTR_ERR(bg->base);

	irq = platform_get_irq(pdev, 0);
	if (irq < 0)
		return irq;

	device_property_read_u32(dev, "ngpios", &ngpios);
	if (!ngpios || ngpios > 32)
		return dev_err_probe(dev, -EINVAL, "invalid ngpios %u\n", ngpios);

	raw_spin_lock_init(&bg->lock);

	/* Quiesce: all interrupts off, stale latches cleared. */
	writel(0, bg->base + BCMRING_GPIO_IE);
	writel(~0U, bg->base + BCMRING_GPIO_IC);

	bg->gc.label = dev_name(dev);
	bg->gc.parent = dev;
	bg->gc.owner = THIS_MODULE;
	bg->gc.base = -1;
	bg->gc.ngpio = ngpios;
	bg->gc.get_direction = bcmring_gpio_get_direction;
	bg->gc.direction_input = bcmring_gpio_direction_input;
	bg->gc.direction_output = bcmring_gpio_direction_output;
	bg->gc.get = bcmring_gpio_get;
	bg->gc.set = bcmring_gpio_set;

	/*
	 * The vendor gpio_bcmring_request() refuses a line whose pin-mux
	 * nibble is not 0 (GPIO).  With "gpio-ranges" pointing at the
	 * pinctrl-single node, requesting a line switches its mux to GPIO.
	 */
	if (device_property_present(dev, "gpio-ranges")) {
		bg->gc.request = gpiochip_generic_request;
		bg->gc.free = gpiochip_generic_free;
	}

	girq = &bg->gc.irq;
	gpio_irq_chip_set_chip(girq, &bcmring_gpio_irq_chip);
	girq->parent_handler = bcmring_gpio_irq_handler;
	girq->num_parents = 1;
	girq->parents = devm_kcalloc(dev, 1, sizeof(*girq->parents),
				     GFP_KERNEL);
	if (!girq->parents)
		return -ENOMEM;
	girq->parents[0] = irq;
	girq->default_type = IRQ_TYPE_NONE;
	girq->handler = handle_bad_irq;

	return devm_gpiochip_add_data(dev, &bg->gc, bg);
}

static const struct of_device_id bcmring_gpio_of_match[] = {
	{ .compatible = "brcm,bcmring-gpio" },
	{ }
};
MODULE_DEVICE_TABLE(of, bcmring_gpio_of_match);

static struct platform_driver bcmring_gpio_driver = {
	.probe = bcmring_gpio_probe,
	.driver = {
		.name = "gpio-bcmring",
		.of_match_table = bcmring_gpio_of_match,
		.suppress_bind_attrs = true,
	},
};
module_platform_driver(bcmring_gpio_driver);

MODULE_DESCRIPTION("Broadcom BCMRING GPIO bank driver");
MODULE_LICENSE("GPL");
