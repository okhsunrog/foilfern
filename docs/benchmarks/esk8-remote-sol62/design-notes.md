# Esk8 Remote schematic

The project has three sheets for power and charging, radio and MCU, and user interfaces.

## Power and startup

- J1 is a 5 V USB-C charge input with independent 5.1 kΩ Rd on CC1 and CC2. Use a dedicated 5 V adapter rated for at least 1.5 A. Fixed Rd supplies attach detection. REG16 lists a 1.5 A input-limit reset value. C1 is 4.7 µF to leave input inrush margin. Verify insertion droop with the chosen source and cable. The connector drawing specifies a nominal 0.8 mm PCB at the mounting edge.
- J2 takes a protected, 4.2 V single-cell Li-ion/LiPo pack with a cell-mounted 10 kΩ NTC. Pin order is BAT, TS, GND. Configure and confirm TS as the charge temperature input with 50 µA excitation and pack-specific limits. The charger path uses SW → L1 → VSYS and a separate VMID bypass. X-Powers Figure 7-1 conflicts with its own power-path block diagram on these pins. M5Stack CoreS3 sheet 1 shows the SW-to-VSYS inductor and independent VMID capacitor and was used to resolve the conflict.
- J3 takes a normally-open momentary switch to ground. It operates PWRON through R6/C6 with the PMIC’s internal pull-up. Validate battery-only key startup, key timing and USB wake on the ordered part.
- U1 DCDC1 powers the MCU and peripherals at nominal 3.3 V. **Procurement gate** — confirm that the eFuse profile enables DCDC1 at startup at 3.3 V, with unused bucks and LDOs disabled and pin 32 in an unused mode. Verify PWRON mode, charge voltage, precharge and fast charge currents, input limit and TS behavior on an unprogrammed sample. REG80/82 and the startup sequence can be factory-configured. C3036461’s programmed profile is unconfirmed. The MCU needs DCDC1 to start before it can communicate with U1.
- L1/L2 are generic land patterns for shielded 1 µH inductors. Select parts with suitable DC bias, target at least 3 A saturation and 2 A RMS, and check final lands, thermal limits and effective MLCC capacitance under bias. DCDC1 is a buck. Validate 3.3 V under load across the intended battery range and inhibit drive commands before the rail falls below the chosen sensor’s operating range.

## MCU and radio

U2 is Nordic **NRF52840-QIAA-R7** in aQFN73. The normal-voltage, LDO-mode wiring follows Nordic configuration 6. All five VDD balls and VDDH share the switched rail. DEC4 and DEC6 share C15. VBUS is grounded for the unused MCU USB interface, and DECUSB, DEC2, DCC and DCCH are open. The die pad, VSS and VSS_PA are grounded. The standard assembly omits C16 on DEC5 for Fxx and later silicon. Fit 820 pF C0G for Dxx and earlier, as specified by Nordic. Select calibrated LFRC in firmware and leave both internal DC/DC enables clear.

Y1 is a 32 MHz, 8 pF, fundamental-mode Abracon ABM11W. Pins 2 and 4 ground the case, and C11/C12 start at Nordic’s 12 pF C0G values. Confirm load and frequency on the assembled board. The RF match starts with 0.8 pF / 4.7 nH / 0.5 pF. Use high-Q RF parts and a 50 Ω feed to J4 for a 2.4 GHz antenna. Copy Nordic’s placement and ground geometry and validate the match with the enclosure and antenna fitted.

## Interfaces

| Function | Contact order or nRF52840 assignment |
| --- | --- |
| PMIC control | SCL P0.26/G1 to U1.40, SDA P0.27/H2 to U1.39, IRQ P0.08/N1 to U1.38. SCL/SDA have 2.2 kΩ pull-ups to 3.3 V, as in CoreS3. IRQ is active low with 4.7 kΩ. Use 7-bit address `0x34` at 100 kHz initially. |
| J6 SSD1306 OLED module | GND, 3.3 V, SCL P0.11/T2, SDA P0.12/U1. Separate 100 kHz bus with 4.7 kΩ pull-ups. Check module supply tolerance, pin order and onboard pull-ups. Expected 7-bit address `0x3C` or `0x3D`. |
| J7 analog trigger | 3.3 V, signal/wiper, GND. P0.02/AIN0 on A12 through 1 kΩ with 100 nF and 100 kΩ to ground. Fit either a 10 kΩ linear potentiometer or a 3.3 V analog ratiometric Hall sensor, such as a TI DRV5055 with local bypass. Use VDD/4 and gain 1/4 for ratiometric sampling. |
| J8–J11 user buttons | Each pair has an active-low input on pin 1 and ground on pin 2. P0.13/AD8, P0.14/AC9, P0.15/AD10, and P0.16/AC11. Use normally-open switches and debounce in firmware. |
| J5 SWD | VTref sense, SWDIO/AC24, GND, SWCLK/AA24, and RESET_N/P0.18/AC13. Enable pin reset in UICR. |

Reserve a low-end fault band above the open-wiper reading when calibrating either trigger. The pulldown biases an open signal low. The transmitter and receiver should inhibit drive on startup, invalid throttle readings, undervoltage and loss of link.

## Acquisition and checks

- Exact LCSC identities are retained for U1 **C3036461**, U2 **C1851953**, J1 **C2765186** and J4 **C88373**. The supplied C9002 crystal was a 12 MHz part. Exact supplier imports for suitable 32 MHz candidates failed, so Y1 uses an independently drawn land pattern from Abracon’s recommended geometry and the datasheet-derived ordering code **ABM11W-32.0000MHZ-8-D1G-T3**. Availability remains unconfirmed. Exact-ID lookup recovered the U.FL import. Inductor searches left generic land patterns. Imported supplier CAD is from the [JLCEDA/EasyEDA Official Library](https://lceda.cn/) ([EasyEDA](https://easyeda.com/)).
- Checked the 74 QIAA pads including the die pad and the 41 AXP2101 pads including EP against manufacturer pin tables. Compared 179 connected device pins across 37 nets against the power, clock, RF, programming and interface assignments. Reviewed full-page renders and close-ups and moved fields away from capacitor artwork. The final schematic reviews found zero annotation overlaps, wire crossings or unwired assigned pins. Native ERC ran with zero messages. Foilfern reports partial coverage because imported roles and required-connection metadata are incomplete.
- PCB layout, antenna selection and tuning, physical footprint and 3D alignment checks, USB hot-plug/ESD validation, battery qualification and hardware bring-up remain for later stages.

## Sources

- Nordic nRF52840 Product Specification v1.11, table 61 and configuration 6, pp. 926–929 and 949–951. The direct documentation site blocked retrieval, so the manufacturer PDF was obtained from the distributor. [Nordic reference circuitry](https://docs.nordicsemi.com/r/bundle/ps_nrf52840/page/ref_circuitry.html) · [Nordic PDF via LCSC](https://datasheet.lcsc.com/datasheet/pdf/bed1abb9fbc0bd0d38d588639838e545.pdf)
- X-Powers AXP2101 datasheet, table 4-1, sections 6.2–6.7 and registers 16, 50, 80 and 82. M5Stack CoreS3 schematic sheets 1 and 3 corroborate the charger wiring and 3.3 V I²C pull-ups. [X-Powers PDF via LCSC](https://datasheet.lcsc.com/datasheet/pdf/4f32bb3c83a61e426b4e09c536b24324.pdf) · [M5Stack CoreS3 schematic](https://m5stack-doc.oss-cn-shenzhen.aliyuncs.com/490/Sch_M5_CoreS3_v1.0.pdf)
- Abracon ABM11W electrical limits, order-code table and land pattern. [Abracon datasheet](https://abracon.com/Resonators/ABM11W.pdf)
- Shou Han TYPE-C 16PIN 2MD(073) pad map and recommended PCB geometry. [Shou Han drawing via LCSC](https://datasheet.lcsc.com/datasheet/pdf/2d53d8d8199c9dd6666d9bc9ac7c3a1e.pdf)
- Solomon Systech SSD1306 I²C interface and TI DRV5055 Hall supply and output behavior. [Solomon PDF via Adafruit](https://cdn-shop.adafruit.com/datasheets/SSD1306.pdf) · [TI DRV5055 datasheet](https://www.ti.com/lit/ds/symlink/drv5055.pdf)
