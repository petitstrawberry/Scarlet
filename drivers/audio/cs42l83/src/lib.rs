#![no_std]
#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use scarlet::sync::Mutex;

use scarlet::{
    device::{
        DeviceInfo,
        audio::{AUDIO_PCM_FORMAT_S16LE, AUDIO_PCM_FORMAT_S32LE, AudioCodec, AudioPcmParams},
        gpio::GpioController,
        i2c::{I2cAddress, I2cBus, I2cError, I2cMessage},
        manager::{DeviceManager, DriverPriority, probe_defer},
        platform::{PlatformDeviceDriver, PlatformDeviceInfo},
    },
    early_println,
    time::udelay,
};

const CS42L83_MAX_7BIT_ADDRESS: usize = 0x7f;
const CS42L83_SOUND_DAI_CELLS: usize = 0;
const CS42L83_CHIP_ID: u32 = 0x42a83;
const CS42L83_BOOT_TIME_US: u64 = 3_000;
const CS42L83_RESET_LOW_TIME_US: u64 = 10;
const CS42L83_I2C_SETTLE_US: u64 = 250;
const CS42L83_HP_ADC_EN_TIME_US: u64 = 20_000;
const CS42L83_PLL_DIVOUT_TIME_US: u64 = 800;
const CS42L83_CLOCK_SWITCH_DELAY_US: u64 = 150;
const CS42L83_PLL_LOCK_POLL_US: u64 = 250;
const CS42L83_PLL_LOCK_TIMEOUT_US: u64 = 1_250;
const CS42L83_SAFE_MIXER_VOLUME: u8 = 0x1e;

const CS42L42_PAGE_REGISTER: u8 = 0x00;

const CS42L42_DEVID_AB: u16 = 0x1001;
const CS42L42_REVID: u16 = 0x1005;
const CS42L42_MCLK_CTL: u16 = 0x1009;
const CS42L42_INTERNAL_FS_MASK: u8 = 1 << 1;

const CS42L42_PWR_CTL1: u16 = 0x1101;
const CS42L42_ASP_DAI_PDN_MASK: u8 = 1 << 6;
const CS42L42_MIXER_PDN_MASK: u8 = 1 << 5;
const CS42L42_EQ_PDN_MASK: u8 = 1 << 4;
const CS42L42_HP_PDN_MASK: u8 = 1 << 3;
const CS42L42_ADC_PDN_MASK: u8 = 1 << 2;
const CS42L42_ASP_DAO_PDN_MASK: u8 = 1 << 7;
const CS42L42_PDN_ALL_MASK: u8 = 1 << 0;
const CS42L42_INIT_POWER_MASK: u8 = CS42L42_ASP_DAO_PDN_MASK
    | CS42L42_ASP_DAI_PDN_MASK
    | CS42L42_MIXER_PDN_MASK
    | CS42L42_EQ_PDN_MASK
    | CS42L42_HP_PDN_MASK
    | CS42L42_ADC_PDN_MASK
    | CS42L42_PDN_ALL_MASK;
const CS42L42_INIT_POWER_VALUE: u8 = CS42L42_ASP_DAO_PDN_MASK
    | CS42L42_ASP_DAI_PDN_MASK
    | CS42L42_MIXER_PDN_MASK
    | CS42L42_EQ_PDN_MASK
    | CS42L42_HP_PDN_MASK
    | CS42L42_ADC_PDN_MASK;
const CS42L42_PLAYBACK_POWER_MASK: u8 =
    CS42L42_ASP_DAI_PDN_MASK | CS42L42_MIXER_PDN_MASK | CS42L42_HP_PDN_MASK;

const CS42L42_PWR_CTL2: u16 = 0x1102;
const CS42L42_DAC_SRC_PDNB_MASK: u8 = 1 << 1;

const CS42L42_OSC_SWITCH: u16 = 0x1107;
const CS42L42_SCLK_PRESENT_MASK: u8 = 1 << 0;

const CS42L42_MCLK_SRC_SEL: u16 = 0x1201;
const CS42L42_MCLK_SRC_SEL_MASK: u8 = 1 << 0;
const CS42L42_FSYNC_PW_LOWER: u16 = 0x1203;
const CS42L42_FSYNC_PW_UPPER: u16 = 0x1204;
const CS42L42_FSYNC_P_LOWER: u16 = 0x1205;
const CS42L42_FSYNC_P_UPPER: u16 = 0x1206;
const CS42L42_ASP_CLK_CFG: u16 = 0x1207;
const CS42L42_ASP_SCLK_EN_MASK: u8 = 1 << 5;
const CS42L42_ASP_MODE_MASK: u8 = 1 << 4;
const CS42L42_ASP_SCPOL_MASK: u8 = 3 << 2;
const CS42L42_ASP_LCPOL_MASK: u8 = 3;
const CS42L42_ASP_LCPOL_INV: u8 = 3;
const CS42L42_ASP_FRM_CFG: u16 = 0x1208;
const CS42L42_ASP_STP_MASK: u8 = 1 << 4;
const CS42L42_ASP_5050_MASK: u8 = 1 << 3;
const CS42L42_ASP_FSD_MASK: u8 = 7;
const CS42L42_ASP_FSD_1_0: u8 = 2;
const CS42L42_FS_RATE_EN: u16 = 0x1209;
const CS42L42_FS_EN_MASK: u8 = 0x0f;
const CS42L42_FS_EN_IASRC_96K: u8 = 0x1;
const CS42L42_FS_EN_OASRC_96K: u8 = 0x2;
const CS42L42_IN_ASRC_CLK: u16 = 0x120a;
const CS42L42_CLK_IASRC_SEL_MASK: u8 = 1;
const CS42L42_OUT_ASRC_CLK: u16 = 0x120b;
const CS42L42_CLK_OASRC_SEL_MASK: u8 = 1;
const CS42L42_PLL_DIV_CFG1: u16 = 0x120c;
const CS42L42_SCLK_PREDIV_MASK: u8 = 3;

const CS42L42_PLL_LOCK_STATUS: u16 = 0x130e;
const CS42L42_ADC_OVFL_INT_MASK: u16 = 0x1316;
const CS42L42_MIXER_INT_MASK: u16 = 0x1317;
const CS42L42_SRC_INT_MASK: u16 = 0x1318;
const CS42L42_ASP_RX_INT_MASK: u16 = 0x1319;
const CS42L42_ASP_TX_INT_MASK: u16 = 0x131a;
const CS42L42_CODEC_INT_MASK: u16 = 0x131b;
const CS42L42_SRCPL_INT_MASK: u16 = 0x131c;
const CS42L42_VPMON_INT_MASK: u16 = 0x131e;
const CS42L42_PLL_LOCK_INT_MASK: u16 = 0x131f;
const CS42L42_TSRS_PLUG_INT_MASK: u16 = 0x1320;

const CS42L42_PLL_CTL1: u16 = 0x1501;
const CS42L42_PLL_START_MASK: u8 = 1;
const CS42L42_PLL_DIV_FRAC0: u16 = 0x1502;
const CS42L42_PLL_DIV_FRAC1: u16 = 0x1503;
const CS42L42_PLL_DIV_FRAC2: u16 = 0x1504;
const CS42L42_PLL_DIV_INT: u16 = 0x1505;
const CS42L42_PLL_CTL3: u16 = 0x1508;
const CS42L42_PLL_CAL_RATIO: u16 = 0x150a;
const CS42L42_PLL_CTL4: u16 = 0x151b;

const CS42L42_HP_CTL: u16 = 0x2001;
const CS42L42_HP_ANA_BMUTE_MASK: u8 = 1 << 3;
const CS42L42_HP_ANA_AMUTE_MASK: u8 = 1 << 2;
const CS42L42_HP_MUTE_MASK: u8 = CS42L42_HP_ANA_BMUTE_MASK | CS42L42_HP_ANA_AMUTE_MASK;

const CS42L42_MIXER_CHA_VOL: u16 = 0x2301;
const CS42L42_MIXER_CHB_VOL: u16 = 0x2303;
const CS42L42_SP_RX_CH_SEL: u16 = 0x2501;
const CS42L42_SP_RX_CHB_SEL_MASK: u8 = 3 << 2;

const CS42L42_ASP_RX_DAI0_EN: u16 = 0x2a01;
const CS42L42_ASP_RX0_CH_EN_MASK: u8 = 0x0f << 2;
const CS42L42_ASP_RX0_CH1_EN: u8 = 1 << 2;
const CS42L42_ASP_RX0_CH2_EN: u8 = 1 << 3;
const CS42L42_ASP_RX_DAI0_CH1_AP_RES: u16 = 0x2a02;
const CS42L42_ASP_RX_DAI0_CH2_AP_RES: u16 = 0x2a05;
const CS42L42_ASP_RX_CH_AP_MASK: u8 = 1 << 6;
const CS42L42_ASP_RX_CH_AP_HI: u8 = 1 << 6;
const CS42L42_ASP_RX_CH_RES_MASK: u8 = 3;
const CS42L42_ASP_RX_CH_RES_16: u8 = 1;
const CS42L42_ASP_RX_CH_RES_32: u8 = 3;

#[derive(Clone, Copy)]
struct Cs42l83PllParams {
    sclk: u32,
    mclk_src_sel: u8,
    sclk_prediv: u8,
    pll_div_int: u8,
    pll_div_frac: u32,
    pll_mode: u8,
    pll_divout: u8,
    mclk_int: u32,
    pll_cal_ratio: u8,
    n: u8,
}

const PLL_RATIO_TABLE: &[Cs42l83PllParams] = &[
    Cs42l83PllParams {
        sclk: 1_411_200,
        mclk_src_sel: 1,
        sclk_prediv: 0x00,
        pll_div_int: 0x80,
        pll_div_frac: 0x000000,
        pll_mode: 0x03,
        pll_divout: 0x10,
        mclk_int: 11_289_600,
        pll_cal_ratio: 128,
        n: 2,
    },
    Cs42l83PllParams {
        sclk: 1_536_000,
        mclk_src_sel: 1,
        sclk_prediv: 0x00,
        pll_div_int: 0x7d,
        pll_div_frac: 0x000000,
        pll_mode: 0x03,
        pll_divout: 0x10,
        mclk_int: 12_000_000,
        pll_cal_ratio: 125,
        n: 2,
    },
    Cs42l83PllParams {
        sclk: 2_304_000,
        mclk_src_sel: 1,
        sclk_prediv: 0x00,
        pll_div_int: 0x55,
        pll_div_frac: 0xc00000,
        pll_mode: 0x02,
        pll_divout: 0x10,
        mclk_int: 12_288_000,
        pll_cal_ratio: 85,
        n: 2,
    },
    Cs42l83PllParams {
        sclk: 2_400_000,
        mclk_src_sel: 1,
        sclk_prediv: 0x00,
        pll_div_int: 0x50,
        pll_div_frac: 0x000000,
        pll_mode: 0x03,
        pll_divout: 0x10,
        mclk_int: 12_000_000,
        pll_cal_ratio: 80,
        n: 2,
    },
    Cs42l83PllParams {
        sclk: 2_822_400,
        mclk_src_sel: 1,
        sclk_prediv: 0x00,
        pll_div_int: 0x40,
        pll_div_frac: 0x000000,
        pll_mode: 0x03,
        pll_divout: 0x10,
        mclk_int: 11_289_600,
        pll_cal_ratio: 128,
        n: 1,
    },
    Cs42l83PllParams {
        sclk: 3_000_000,
        mclk_src_sel: 1,
        sclk_prediv: 0x00,
        pll_div_int: 0x40,
        pll_div_frac: 0x000000,
        pll_mode: 0x03,
        pll_divout: 0x10,
        mclk_int: 12_000_000,
        pll_cal_ratio: 128,
        n: 1,
    },
    Cs42l83PllParams {
        sclk: 3_072_000,
        mclk_src_sel: 1,
        sclk_prediv: 0x00,
        pll_div_int: 0x3e,
        pll_div_frac: 0x800000,
        pll_mode: 0x03,
        pll_divout: 0x10,
        mclk_int: 12_000_000,
        pll_cal_ratio: 125,
        n: 1,
    },
    Cs42l83PllParams {
        sclk: 4_000_000,
        mclk_src_sel: 1,
        sclk_prediv: 0x00,
        pll_div_int: 0x30,
        pll_div_frac: 0x800000,
        pll_mode: 0x03,
        pll_divout: 0x10,
        mclk_int: 12_000_000,
        pll_cal_ratio: 96,
        n: 1,
    },
    Cs42l83PllParams {
        sclk: 4_096_000,
        mclk_src_sel: 1,
        sclk_prediv: 0x00,
        pll_div_int: 0x2e,
        pll_div_frac: 0xe00000,
        pll_mode: 0x03,
        pll_divout: 0x10,
        mclk_int: 12_000_000,
        pll_cal_ratio: 94,
        n: 1,
    },
    Cs42l83PllParams {
        sclk: 4_800_000,
        mclk_src_sel: 1,
        sclk_prediv: 0x01,
        pll_div_int: 0x50,
        pll_div_frac: 0x000000,
        pll_mode: 0x03,
        pll_divout: 0x10,
        mclk_int: 12_000_000,
        pll_cal_ratio: 80,
        n: 2,
    },
    Cs42l83PllParams {
        sclk: 5_644_800,
        mclk_src_sel: 1,
        sclk_prediv: 0x01,
        pll_div_int: 0x40,
        pll_div_frac: 0x000000,
        pll_mode: 0x03,
        pll_divout: 0x10,
        mclk_int: 11_289_600,
        pll_cal_ratio: 128,
        n: 1,
    },
    Cs42l83PllParams {
        sclk: 6_000_000,
        mclk_src_sel: 1,
        sclk_prediv: 0x01,
        pll_div_int: 0x40,
        pll_div_frac: 0x000000,
        pll_mode: 0x03,
        pll_divout: 0x10,
        mclk_int: 12_000_000,
        pll_cal_ratio: 128,
        n: 1,
    },
    Cs42l83PllParams {
        sclk: 6_144_000,
        mclk_src_sel: 1,
        sclk_prediv: 0x01,
        pll_div_int: 0x3e,
        pll_div_frac: 0x800000,
        pll_mode: 0x03,
        pll_divout: 0x10,
        mclk_int: 12_000_000,
        pll_cal_ratio: 125,
        n: 1,
    },
    Cs42l83PllParams {
        sclk: 9_600_000,
        mclk_src_sel: 1,
        sclk_prediv: 0x02,
        pll_div_int: 0x50,
        pll_div_frac: 0x000000,
        pll_mode: 0x03,
        pll_divout: 0x10,
        mclk_int: 12_000_000,
        pll_cal_ratio: 80,
        n: 2,
    },
    Cs42l83PllParams {
        sclk: 11_289_600,
        mclk_src_sel: 0,
        sclk_prediv: 0,
        pll_div_int: 0,
        pll_div_frac: 0,
        pll_mode: 0,
        pll_divout: 0,
        mclk_int: 11_289_600,
        pll_cal_ratio: 0,
        n: 1,
    },
    Cs42l83PllParams {
        sclk: 12_000_000,
        mclk_src_sel: 0,
        sclk_prediv: 0,
        pll_div_int: 0,
        pll_div_frac: 0,
        pll_mode: 0,
        pll_divout: 0,
        mclk_int: 12_000_000,
        pll_cal_ratio: 0,
        n: 1,
    },
    Cs42l83PllParams {
        sclk: 12_288_000,
        mclk_src_sel: 0,
        sclk_prediv: 0,
        pll_div_int: 0,
        pll_div_frac: 0,
        pll_mode: 0,
        pll_divout: 0,
        mclk_int: 12_288_000,
        pll_cal_ratio: 0,
        n: 1,
    },
    Cs42l83PllParams {
        sclk: 19_200_000,
        mclk_src_sel: 1,
        sclk_prediv: 0x03,
        pll_div_int: 0x50,
        pll_div_frac: 0x000000,
        pll_mode: 0x03,
        pll_divout: 0x10,
        mclk_int: 12_000_000,
        pll_cal_ratio: 80,
        n: 2,
    },
    Cs42l83PllParams {
        sclk: 22_579_200,
        mclk_src_sel: 1,
        sclk_prediv: 0x03,
        pll_div_int: 0x40,
        pll_div_frac: 0x000000,
        pll_mode: 0x03,
        pll_divout: 0x10,
        mclk_int: 11_289_600,
        pll_cal_ratio: 128,
        n: 1,
    },
    Cs42l83PllParams {
        sclk: 24_000_000,
        mclk_src_sel: 1,
        sclk_prediv: 0x03,
        pll_div_int: 0x40,
        pll_div_frac: 0x000000,
        pll_mode: 0x03,
        pll_divout: 0x10,
        mclk_int: 12_000_000,
        pll_cal_ratio: 128,
        n: 1,
    },
    Cs42l83PllParams {
        sclk: 24_576_000,
        mclk_src_sel: 1,
        sclk_prediv: 0x03,
        pll_div_int: 0x40,
        pll_div_frac: 0x000000,
        pll_mode: 0x03,
        pll_divout: 0x10,
        mclk_int: 12_288_000,
        pll_cal_ratio: 128,
        n: 1,
    },
];

static CS42L83_CODECS: Mutex<Vec<Arc<Cs42l83>>> = Mutex::new(Vec::new());

struct CsGpio {
    controller: Arc<dyn GpioController>,
    pin: u32,
    active_low: bool,
}

impl CsGpio {
    fn set_output(&self, active: bool) {
        self.controller
            .set_direction_output(self.pin, self.physical_value(active));
    }

    fn set(&self, active: bool) {
        self.controller
            .set_value(self.pin, self.physical_value(active));
    }

    fn physical_value(&self, active: bool) -> bool {
        if self.active_low { !active } else { active }
    }
}

struct Cs42l83 {
    bus: Arc<dyn I2cBus>,
    address: I2cAddress,
    bus_phandle: u32,
    reset_gpio: Option<CsGpio>,
    page: Mutex<u8>,
    pll_config: Mutex<Option<Cs42l83PllParams>>,
    powered: Mutex<bool>,
    stream_active: Mutex<bool>,
}

impl Cs42l83 {
    fn new(
        bus: Arc<dyn I2cBus>,
        address: I2cAddress,
        bus_phandle: u32,
        reset_gpio: Option<CsGpio>,
    ) -> Self {
        Self {
            bus,
            address,
            bus_phandle,
            reset_gpio,
            page: Mutex::new(0),
            pll_config: Mutex::new(None),
            powered: Mutex::new(false),
            stream_active: Mutex::new(false),
        }
    }

    fn write_raw(&self, bytes: &[u8]) -> Result<(), I2cError> {
        let mut messages = alloc::vec![I2cMessage::write(self.address, bytes, true)];
        let result = self.bus.transfer(&mut messages);
        udelay(CS42L83_I2C_SETTLE_US);
        result
    }

    fn select_page(&self, page: u8) -> Result<(), I2cError> {
        let mut current = self.page.lock();
        if *current == page {
            return Ok(());
        }
        self.write_raw(&[CS42L42_PAGE_REGISTER, page])?;
        *current = page;
        Ok(())
    }

    fn write_register(&self, register: u16, value: u8) -> Result<(), I2cError> {
        self.select_page((register >> 8) as u8)?;
        self.write_raw(&[(register & 0xff) as u8, value])
    }

    fn read_register(&self, register: u16) -> Result<u8, I2cError> {
        self.select_page((register >> 8) as u8)?;
        let reg = (register & 0xff) as u8;
        let mut messages = alloc::vec![
            I2cMessage::write(self.address, &[reg], false),
            I2cMessage::read(self.address, 1, true),
        ];
        self.bus.transfer(&mut messages)?;
        udelay(CS42L83_I2C_SETTLE_US);
        Ok(messages[1].data[0])
    }

    fn update_bits(&self, register: u16, mask: u8, value: u8) -> Result<(), I2cError> {
        let current = self.read_register(register)?;
        self.write_register(register, (current & !mask) | (value & mask))
    }

    fn read_device_id(&self) -> Result<u32, I2cError> {
        let devid_ab = self.read_register(CS42L42_DEVID_AB)? as u32;
        let devid_cd = self.read_register(CS42L42_DEVID_AB + 1)? as u32;
        let devid_e = self.read_register(CS42L42_DEVID_AB + 2)? as u32;
        Ok((devid_ab << 12) | (devid_cd << 4) | ((devid_e & 0xf0) >> 4))
    }

    fn hardware_reset(&self) {
        if let Some(gpio) = &self.reset_gpio {
            gpio.set_output(false);
            udelay(CS42L83_RESET_LOW_TIME_US);
            gpio.set(true);
        }
        udelay(CS42L83_BOOT_TIME_US);
        *self.page.lock() = 0;
    }

    fn initialize(&self) -> Result<u8, I2cError> {
        self.hardware_reset();
        let device_id = self.read_device_id()?;
        if device_id != CS42L83_CHIP_ID {
            return Err(I2cError::InvalidArg);
        }
        let revision = self.read_register(CS42L42_REVID)?;

        self.update_bits(
            CS42L42_PWR_CTL1,
            CS42L42_INIT_POWER_MASK,
            CS42L42_INIT_POWER_VALUE,
        )?;
        self.mask_interrupts()?;
        self.set_analog_mute(true)?;
        Ok(revision)
    }

    fn mask_interrupts(&self) -> Result<(), I2cError> {
        for register in [
            CS42L42_ADC_OVFL_INT_MASK,
            CS42L42_MIXER_INT_MASK,
            CS42L42_SRC_INT_MASK,
            CS42L42_ASP_RX_INT_MASK,
            CS42L42_ASP_TX_INT_MASK,
            CS42L42_CODEC_INT_MASK,
            CS42L42_SRCPL_INT_MASK,
            CS42L42_VPMON_INT_MASK,
            CS42L42_PLL_LOCK_INT_MASK,
            CS42L42_TSRS_PLUG_INT_MASK,
        ] {
            self.write_register(register, 0xff)?;
        }
        Ok(())
    }

    fn configure_i2s_ib_if(&self) -> Result<(), I2cError> {
        self.update_bits(
            CS42L42_ASP_FRM_CFG,
            CS42L42_ASP_STP_MASK | CS42L42_ASP_5050_MASK | CS42L42_ASP_FSD_MASK,
            CS42L42_ASP_5050_MASK | CS42L42_ASP_FSD_1_0,
        )?;
        self.update_bits(
            CS42L42_ASP_CLK_CFG,
            CS42L42_ASP_MODE_MASK | CS42L42_ASP_SCPOL_MASK | CS42L42_ASP_LCPOL_MASK,
            CS42L42_ASP_LCPOL_INV,
        )
    }

    fn sample_width_value(format: u32) -> Result<u8, I2cError> {
        match format {
            AUDIO_PCM_FORMAT_S16LE => Ok(CS42L42_ASP_RX_CH_RES_16),
            AUDIO_PCM_FORMAT_S32LE => Ok(CS42L42_ASP_RX_CH_RES_32),
            _ => Err(I2cError::InvalidArg),
        }
    }

    fn configure_playback_hw(
        &self,
        params: &AudioPcmParams,
        tx_mask: u32,
        slots: usize,
        slot_width: usize,
    ) -> Result<(), I2cError> {
        if params.channels == 0 || params.channels > 2 || tx_mask & 0x3 != 0x3 {
            return Err(I2cError::InvalidArg);
        }
        if slots < 2 || slot_width != 32 {
            return Err(I2cError::InvalidArg);
        }

        let bclk = params.rate.checked_mul(64).ok_or(I2cError::InvalidArg)?;
        let width = Self::sample_width_value(params.format)?;

        self.configure_i2s_ib_if()?;
        self.update_bits(
            CS42L42_ASP_RX_DAI0_CH1_AP_RES,
            CS42L42_ASP_RX_CH_AP_MASK | CS42L42_ASP_RX_CH_RES_MASK,
            width,
        )?;
        self.update_bits(
            CS42L42_ASP_RX_DAI0_CH2_AP_RES,
            CS42L42_ASP_RX_CH_AP_MASK | CS42L42_ASP_RX_CH_RES_MASK,
            CS42L42_ASP_RX_CH_AP_HI | width,
        )?;
        self.update_bits(
            CS42L42_SP_RX_CH_SEL,
            CS42L42_SP_RX_CHB_SEL_MASK,
            (params.channels.saturating_sub(1) as u8) << 2,
        )?;
        self.update_bits(
            CS42L42_ASP_RX_DAI0_EN,
            CS42L42_ASP_RX0_CH_EN_MASK,
            CS42L42_ASP_RX0_CH1_EN | CS42L42_ASP_RX0_CH2_EN,
        )?;
        self.write_register(CS42L42_MIXER_CHA_VOL, CS42L83_SAFE_MIXER_VOLUME)?;
        self.write_register(CS42L42_MIXER_CHB_VOL, CS42L83_SAFE_MIXER_VOLUME)?;
        self.configure_pll(bclk, params.rate)?;
        self.configure_asp(bclk, params.rate)?;
        self.configure_src(params.rate)
    }

    fn configure_pll(&self, sclk: u32, sample_rate: u32) -> Result<(), I2cError> {
        let Some(params) = PLL_RATIO_TABLE
            .iter()
            .copied()
            .find(|params| params.sclk == sclk && params.mclk_int % sample_rate == 0)
        else {
            return Err(I2cError::InvalidArg);
        };

        let internal_fs = if params.mclk_int != 12_000_000 && params.mclk_int != 24_000_000 {
            CS42L42_INTERNAL_FS_MASK
        } else {
            0
        };
        self.update_bits(CS42L42_MCLK_CTL, CS42L42_INTERNAL_FS_MASK, internal_fs)?;
        if params.mclk_src_sel == 0 {
            self.update_bits(CS42L42_PLL_CTL1, CS42L42_PLL_START_MASK, 0)?;
        } else {
            self.update_bits(
                CS42L42_PLL_DIV_CFG1,
                CS42L42_SCLK_PREDIV_MASK,
                params.sclk_prediv,
            )?;
            self.write_register(CS42L42_PLL_DIV_INT, params.pll_div_int)?;
            self.write_register(CS42L42_PLL_DIV_FRAC0, (params.pll_div_frac & 0xff) as u8)?;
            self.write_register(
                CS42L42_PLL_DIV_FRAC1,
                ((params.pll_div_frac >> 8) & 0xff) as u8,
            )?;
            self.write_register(
                CS42L42_PLL_DIV_FRAC2,
                ((params.pll_div_frac >> 16) & 0xff) as u8,
            )?;
            self.write_register(CS42L42_PLL_CTL4, params.pll_mode)?;
            self.write_register(CS42L42_PLL_CTL3, params.pll_divout.saturating_mul(params.n))?;
            self.write_register(CS42L42_PLL_CAL_RATIO, params.pll_cal_ratio)?;
        }
        *self.pll_config.lock() = Some(params);
        Ok(())
    }

    fn configure_asp(&self, sclk: u32, sample_rate: u32) -> Result<(), I2cError> {
        let fsync = sclk / sample_rate;
        if fsync * sample_rate != sclk || fsync % 2 != 0 {
            return Err(I2cError::InvalidArg);
        }
        let period = fsync - 1;
        self.write_register(CS42L42_FSYNC_P_LOWER, (period & 0xff) as u8)?;
        self.write_register(CS42L42_FSYNC_P_UPPER, ((period >> 8) & 0xff) as u8)?;

        let pulse_width = fsync / 2 - 1;
        self.write_register(CS42L42_FSYNC_PW_LOWER, (pulse_width & 0xff) as u8)?;
        self.write_register(CS42L42_FSYNC_PW_UPPER, ((pulse_width >> 8) & 0xff) as u8)
    }

    fn configure_src(&self, sample_rate: u32) -> Result<(), I2cError> {
        let fs = if sample_rate <= 48_000 { 0 } else { 1 };
        self.update_bits(
            CS42L42_FS_RATE_EN,
            CS42L42_FS_EN_MASK,
            CS42L42_FS_EN_IASRC_96K | CS42L42_FS_EN_OASRC_96K,
        )?;
        self.update_bits(CS42L42_IN_ASRC_CLK, CS42L42_CLK_IASRC_SEL_MASK, fs)?;
        self.update_bits(CS42L42_OUT_ASRC_CLK, CS42L42_CLK_OASRC_SEL_MASK, fs)
    }

    fn set_analog_mute(&self, muted: bool) -> Result<(), I2cError> {
        self.update_bits(
            CS42L42_HP_CTL,
            CS42L42_HP_MUTE_MASK,
            if muted { CS42L42_HP_MUTE_MASK } else { 0 },
        )
    }

    fn set_powered(&self, powered: bool) -> Result<(), I2cError> {
        let mut current = self.powered.lock();
        if *current == powered {
            return Ok(());
        }

        if powered {
            self.update_bits(CS42L42_PWR_CTL1, CS42L42_PLAYBACK_POWER_MASK, 0)?;
            self.update_bits(
                CS42L42_PWR_CTL2,
                CS42L42_DAC_SRC_PDNB_MASK,
                CS42L42_DAC_SRC_PDNB_MASK,
            )?;
            self.update_bits(
                CS42L42_ASP_CLK_CFG,
                CS42L42_ASP_SCLK_EN_MASK,
                CS42L42_ASP_SCLK_EN_MASK,
            )?;
            udelay(CS42L83_HP_ADC_EN_TIME_US);
        } else {
            self.set_analog_mute(true)?;
            self.update_bits(CS42L42_ASP_CLK_CFG, CS42L42_ASP_SCLK_EN_MASK, 0)?;
            self.update_bits(CS42L42_PWR_CTL2, CS42L42_DAC_SRC_PDNB_MASK, 0)?;
            self.update_bits(
                CS42L42_PWR_CTL1,
                CS42L42_PLAYBACK_POWER_MASK,
                CS42L42_PLAYBACK_POWER_MASK,
            )?;
        }
        *current = powered;
        Ok(())
    }

    fn set_muted(&self, muted: bool) -> Result<(), I2cError> {
        if muted {
            self.set_analog_mute(true)?;
            let mut active = self.stream_active.lock();
            if *active {
                self.write_register(CS42L42_OSC_SWITCH, 0)?;
                udelay(CS42L83_CLOCK_SWITCH_DELAY_US);
                self.update_bits(CS42L42_MCLK_SRC_SEL, CS42L42_MCLK_SRC_SEL_MASK, 0)?;
                udelay(100);
                self.update_bits(CS42L42_PLL_CTL1, CS42L42_PLL_START_MASK, 0)?;
                *active = false;
            }
            return Ok(());
        }

        let is_powered = *self.powered.lock();
        if !is_powered {
            self.set_powered(true)?;
        }

        let mut active = self.stream_active.lock();
        if !*active {
            let params = (*self.pll_config.lock()).ok_or(I2cError::InvalidArg)?;
            if params.mclk_src_sel != 0 {
                self.update_bits(
                    CS42L42_PLL_CTL1,
                    CS42L42_PLL_START_MASK,
                    CS42L42_PLL_START_MASK,
                )?;
                if params.n > 1 {
                    udelay(CS42L83_PLL_DIVOUT_TIME_US);
                    self.write_register(CS42L42_PLL_CTL3, params.pll_divout)?;
                }
                let mut elapsed = 0;
                while elapsed < CS42L83_PLL_LOCK_TIMEOUT_US {
                    if self.read_register(CS42L42_PLL_LOCK_STATUS).unwrap_or(0) & 1 != 0 {
                        break;
                    }
                    udelay(CS42L83_PLL_LOCK_POLL_US);
                    elapsed += CS42L83_PLL_LOCK_POLL_US;
                }
                self.update_bits(
                    CS42L42_MCLK_SRC_SEL,
                    CS42L42_MCLK_SRC_SEL_MASK,
                    CS42L42_MCLK_SRC_SEL_MASK,
                )?;
            }
            self.write_register(CS42L42_OSC_SWITCH, CS42L42_SCLK_PRESENT_MASK)?;
            udelay(CS42L83_CLOCK_SWITCH_DELAY_US);
            *active = true;
        }
        self.set_analog_mute(false)
    }
}

impl AudioCodec for Cs42l83 {
    fn configure_playback(
        &self,
        params: &AudioPcmParams,
        tx_mask: u32,
        slots: usize,
        slot_width: usize,
    ) -> Result<(), &'static str> {
        self.configure_playback_hw(params, tx_mask, slots, slot_width)
            .map_err(|_| "cs42l83: failed to configure playback")
    }

    fn set_playback_muted(&self, muted: bool) -> Result<(), &'static str> {
        self.set_muted(muted)
            .map_err(|_| "cs42l83: failed to change mute state")
    }

    fn set_playback_powered(&self, powered: bool) -> Result<(), &'static str> {
        self.set_powered(powered)
            .map_err(|_| "cs42l83: failed to change power state")
    }
}

fn read_i2c_address(device: &PlatformDeviceInfo) -> Result<I2cAddress, &'static str> {
    let address = device
        .property("reg")
        .and_then(|property| property.as_usize())
        .ok_or("cs42l83: missing I2C address")?;
    if address > CS42L83_MAX_7BIT_ADDRESS {
        return Err("cs42l83: unsupported I2C address");
    }

    Ok(I2cAddress::SevenBit(address as u8))
}

fn read_sound_dai_cells(device: &PlatformDeviceInfo) -> Result<usize, &'static str> {
    let cells = device
        .property("#sound-dai-cells")
        .and_then(|property| property.as_usize())
        .unwrap_or(CS42L83_SOUND_DAI_CELLS);
    if cells != CS42L83_SOUND_DAI_CELLS {
        return Err("cs42l83: unsupported #sound-dai-cells");
    }

    Ok(cells)
}

fn read_phandle(device: &PlatformDeviceInfo) -> Result<u32, &'static str> {
    device
        .property("phandle")
        .or_else(|| device.property("linux,phandle"))
        .and_then(|property| property.as_usize())
        .map(|value| value as u32)
        .ok_or("cs42l83: missing phandle")
}

fn read_be_u32_cells(value: &[u8]) -> impl Iterator<Item = u32> + '_ {
    value
        .chunks_exact(4)
        .map(|chunk| u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
}

fn resolve_gpio(device: &PlatformDeviceInfo, name: &str) -> Result<Option<CsGpio>, &'static str> {
    let Some(property) = device.property(name) else {
        return Ok(None);
    };
    let mut cells = read_be_u32_cells(property.value());
    let phandle = cells.next().ok_or("cs42l83: malformed GPIO property")?;
    let pin = cells.next().ok_or("cs42l83: malformed GPIO property")?;
    let flags = cells.next().unwrap_or(0);
    match DeviceManager::get_manager().get_gpio_controller(phandle) {
        Some(controller) => Ok(Some(CsGpio {
            controller,
            pin,
            active_low: flags & 1 != 0,
        })),
        None => {
            early_println!(
                "[cs42l83] GPIO controller phandle {:#x} for {} is not ready, deferring",
                phandle,
                name
            );
            probe_defer()
        }
    }
}

fn resolve_i2c_bus(device: &PlatformDeviceInfo) -> Result<(u32, Arc<dyn I2cBus>), &'static str> {
    let bus_phandle = device
        .parent_phandle()
        .ok_or("cs42l83: missing parent I2C bus")?;
    match DeviceManager::get_manager().get_i2c_bus(bus_phandle) {
        Some(bus) => Ok((bus_phandle, bus)),
        None => {
            early_println!(
                "[cs42l83] I2C bus phandle {:#x} is not ready, deferring",
                bus_phandle
            );
            probe_defer()
        }
    }
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let (bus_phandle, bus) = resolve_i2c_bus(device)?;
    let phandle = read_phandle(device)?;
    let address = read_i2c_address(device)?;
    let _sound_dai_cells = read_sound_dai_cells(device)?;
    let reset_gpio = resolve_gpio(device, "reset-gpios")?;
    let has_reset_gpio = reset_gpio.is_some();
    let codec = Arc::new(Cs42l83::new(bus, address, bus_phandle, reset_gpio));
    let revision = codec.initialize().map_err(|_| {
        early_println!(
            "[cs42l83] initialization failed for phandle={:#x}, addr={:#x}",
            phandle,
            address.raw()
        );
        "cs42l83: codec initialization failed"
    })?;

    let audio_codec: Arc<dyn AudioCodec> = codec.clone();
    DeviceManager::get_manager().register_audio_codec(phandle, audio_codec);
    CS42L83_CODECS.lock().push(codec);

    early_println!(
        "[cs42l83] registered {} at phandle={:#x}, bus-phandle={:#x}, addr={:#x}, revision={:#x}, reset-gpio={}",
        device.name(),
        phandle,
        bus_phandle,
        address.raw(),
        revision,
        has_reset_gpio
    );

    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_driver() {
    let driver = PlatformDeviceDriver::new(
        "cs42l83",
        probe_fn,
        remove_fn,
        alloc::vec!["cirrus,cs42l83"],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Standard);
}

scarlet::driver_initcall!(register_driver);

#[used]
static SCARLET_DRIVER_CS42L83_ANCHOR: fn() = force_link;

/// Keep the external driver object linked into Scarlet module builds.
#[inline(never)]
pub fn force_link() {}
