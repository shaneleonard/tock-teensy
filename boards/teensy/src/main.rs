#![no_std]
#![no_main]
#![feature(asm,const_fn,lang_items,compiler_builtins_lib)]

extern crate capsules;
extern crate compiler_builtins;

#[macro_use(debug, static_init)]
extern crate kernel;

#[macro_use]
extern crate common;

#[allow(dead_code)]
extern crate mk66;

#[macro_use]
pub mod io;

#[allow(dead_code)]
mod tests;

use capsules::virtual_spi::{VirtualSpiMasterDevice, MuxSpiMaster};

use capsules::alarm::AlarmDriver;
use capsules::spi::Spi;
use capsules::gpio::GPIO;
use capsules::led::{ActivationMode, LED};
use kernel::hil::spi::SpiMaster;
use kernel::hil::uart::UART;
pub mod xconsole;

#[allow(unused)]
struct Teensy {
    xconsole: &'static xconsole::XConsole<'static, mk66::uart::Uart>,
    gpio: &'static GPIO<'static, mk66::gpio::Gpio<'static>>,
    led: &'static LED<'static, mk66::gpio::Gpio<'static>>,
    alarm: &'static AlarmDriver<'static, mk66::pit::Pit<'static>>,
    spi: &'static Spi<'static, VirtualSpiMasterDevice<'static, mk66::spi::Spi<'static>>>,
    ipc: kernel::ipc::IPC,
}

impl kernel::Platform for Teensy {
    fn with_driver<F, R>(&self, driver_num: usize, f: F) -> R
        where F: FnOnce(Option<&kernel::Driver>) -> R
    {
        match driver_num {
            xconsole::DRIVER_NUM => f(Some(self.xconsole)),
            capsules::gpio::DRIVER_NUM => f(Some(self.gpio)),

            capsules::alarm::DRIVER_NUM => f(Some(self.alarm)),
            capsules::spi::DRIVER_NUM => f(Some(self.spi)),

            capsules::led::DRIVER_NUM => f(Some(self.led)),

            kernel::ipc::DRIVER_NUM => f(Some(&self.ipc)),
            _ => f(None),
        }
    }
}

#[link_section = ".flashconfig"]
#[no_mangle]
pub static FLASH_CONFIG_BYTES: [u8; 16] = [
    0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    0xFF, 0xFF, 0xFF, 0xFF, 0xDE, 0xF9, 0xFF, 0xFF,
];

pub unsafe fn set_pin_primary_functions() {
    use mk66::gpio::functions::*;
    use mk66::gpio::*;
    PB17.claim_as(UART0_TX);
    PB16.claim_as(UART0_RX);

    PD05.claim_as(SPI1_SCK);
    PD06.claim_as(SPI1_MOSI);
}

#[no_mangle]
pub unsafe fn reset_handler() {
    use mk66::{clock, wdog, sim, pit, spi, uart};
    use mk66::sim::Clock;
    use mk66::gpio::*;

    // Disable the watchdog.
    wdog::stop();

    // Relocate the text and data segments.
    mk66::init();

    // Configure the system clock.
    clock::configure(120);

    // Enable the Port Control and Interrupt clocks.
    sim::clocks::PORTABCDE.enable();

    pit::PIT.init();
    spi::SPI1.init();

    set_pin_primary_functions();

    let xconsole = static_init!(
            xconsole::XConsole<uart::Uart>,
            xconsole::XConsole::new(&uart::UART0,
                                    115200,
                                    &mut xconsole::WRITE_BUF,
                                    &mut xconsole::READ_BUF,
                                    kernel::Grant::create())
    );
    uart::UART0.set_client(xconsole);
    xconsole.initialize();

    let kc = static_init!(
            xconsole::App,
            xconsole::App::default()
        );
    kernel::debug::assign_console_driver(Some(xconsole), kc);

    let alarm = static_init!(
            AlarmDriver<'static, mk66::pit::Pit>,
            AlarmDriver::new(&pit::PIT,
                             kernel::Grant::create())
        );
    pit::PIT.set_client(alarm);

    let mux_spi = static_init!(
            MuxSpiMaster<'static, spi::Spi<'static>>,
            MuxSpiMaster::new(&spi::SPI1)
        );

    spi::SPI1.set_client(mux_spi);

    let virtual_spi = static_init!(
            VirtualSpiMasterDevice<'static, spi::Spi<'static>>,
            VirtualSpiMasterDevice::new(mux_spi, 0)
        );

    let spi = static_init!(
            capsules::spi::Spi<'static, VirtualSpiMasterDevice<'static, spi::Spi<'static>>>,
            capsules::spi::Spi::new(virtual_spi)
        );

    static mut SPI_READ_BUF: [u8; 1024] = [0; 1024];
    static mut SPI_WRITE_BUF: [u8; 1024] = [0; 1024];
    spi.config_buffers(&mut SPI_READ_BUF, &mut SPI_WRITE_BUF);
    virtual_spi.set_client(spi);

    let gpio_pins = static_init!(
        [&'static Gpio; 8],
        [PD01.claim_as_gpio(),
         PC00.claim_as_gpio(),
         PB00.claim_as_gpio(),
         PB01.claim_as_gpio(),
         PB03.claim_as_gpio(),
         PB02.claim_as_gpio(),
         // PD05.claim_as_gpio(),
         // PD06.claim_as_gpio(),
         PC01.claim_as_gpio(),
         PC02.claim_as_gpio()]
        );

    let gpio = static_init!(
        GPIO<'static, Gpio>,
        GPIO::new(gpio_pins)
        );

    for pin in gpio_pins.iter() {
        pin.set_client(gpio);
    }

    let led_pins = static_init!(
        [(&'static Gpio, ActivationMode); 1],
        [(PC05.claim_as_gpio(), ActivationMode::ActiveHigh)]
        );

    let led = static_init!(
        LED<'static, Gpio>,
        LED::new(led_pins)
        );

    let teensy = Teensy {
        xconsole: xconsole,
        gpio: gpio,
        led: led,
        alarm: alarm,
        spi: spi,
        ipc: kernel::ipc::IPC::new(),
    };

    let mut chip = mk66::chip::MK66::new();
    uart::UART0.enable_rx();
    uart::UART0.enable_rx_interrupts();

    if tests::TEST {
        tests::test();
    }
    kernel::main(&teensy, &mut chip, load_processes(), &teensy.ipc);
}


unsafe fn load_processes() -> &'static mut [Option<kernel::Process<'static>>] {
    extern "C" {
        /// Beginning of the ROM region containing the app images.
        static _sapps: u8;
    }

    const NUM_PROCS: usize = 1;

    // Total memory allocated to the processes
    #[link_section = ".app_memory"]
    static mut APP_MEMORY: [u8; 1 << 17] = [0; 1 << 17];

    // How the kernel responds when a process faults
    const FAULT_RESPONSE: kernel::process::FaultResponse = kernel::process::FaultResponse::Panic;

    static mut PROCESSES: [Option<kernel::Process<'static>>; NUM_PROCS] = [None];

    // Create the processes and allocate the app memory among them
    let mut apps_in_flash_ptr = &_sapps as *const u8;
    let mut app_memory_ptr = APP_MEMORY.as_mut_ptr();
    let mut app_memory_size = APP_MEMORY.len();
    for i in 0..NUM_PROCS {
        let (process, flash_offset, memory_offset) = kernel::Process::create(apps_in_flash_ptr,
                                                                             app_memory_ptr,
                                                                             app_memory_size,
                                                                             FAULT_RESPONSE);
        if process.is_none() {
            break;
        }

        PROCESSES[i] = process;
        apps_in_flash_ptr = apps_in_flash_ptr.offset(flash_offset as isize);
        app_memory_ptr = app_memory_ptr.offset(memory_offset as isize);
        app_memory_size -= memory_offset;
    }

    &mut PROCESSES
}
