use crate::flasher::Command;
use crate::image_format::ImageFormatId;
use crate::partition_table::{SubType, Type};
use crate::Chip;
use miette::{Diagnostic, SourceOffset, SourceSpan};
use slip_codec::Error as SlipError;
use std::fmt::{Display, Formatter};
use std::io;
use strum::{AsStaticRef, VariantNames};
use thiserror::Error;

#[derive(Error, Debug, Diagnostic)]
#[non_exhaustive]
pub enum Error {
    #[error("Error while connecting to device")]
    #[diagnostic(transparent)]
    Connection(#[source] ConnectionError),
    #[error("Communication error while flashing device")]
    #[diagnostic(transparent)]
    Flashing(#[source] ConnectionError),
    #[error("Supplied elf image is not valid")]
    #[diagnostic(
        code(espflash::invalid_elf),
        help("Try running `cargo clean` and rebuilding the image")
    )]
    InvalidElf(#[from] ElfError),
    #[error("Supplied elf image can not be ran from ram as it includes segments mapped to rom addresses")]
    #[diagnostic(
        code(espflash::not_ram_loadable),
        help("Either build the binary to be all in ram or remove the `--ram` option to load the image to flash")
    )]
    ElfNotRamLoadable,
    #[error("The bootloader returned an error")]
    #[diagnostic(transparent)]
    RomError(#[from] RomError),
    #[error("Chip not recognized, supported chip types are esp8266, esp32 and esp32-c3")]
    #[diagnostic(
        code(espflash::unrecognized_chip),
        help("If your chip is supported, try hard-resetting the device and try again")
    )]
    UnrecognizedChip(#[from] ChipDetectError),
    #[error("Flash chip not supported, flash sizes from 1 to 16MB are supported")]
    #[diagnostic(code(espflash::unrecognized_flash))]
    UnsupportedFlash(#[from] FlashDetectError),
    #[error("Failed to connect to on-device flash")]
    #[diagnostic(code(espflash::flash_connect))]
    FlashConnect,
    #[error(transparent)]
    #[diagnostic(transparent)]
    MalformedPartitionTable(#[from] PartitionTableError),
    #[error(transparent)]
    #[diagnostic(transparent)]
    UnsupportedImageFormat(#[from] UnsupportedImageFormatError),
    #[error("Unrecognized image format {0}")]
    #[diagnostic(
        code(espflash::unknown_format),
        help("The following image formats are {}", ImageFormatId::VARIANTS.join(", "))
    )]
    UnknownImageFormat(String),
    #[error("binary is not setup correct to support direct boot")]
    #[diagnostic(
        code(espflash::invalid_direct_boot),
        help(
            "See the following page for documentation on how to setup your binary for direct boot:
https://github.com/espressif/esp32c3-direct-boot-example"
        )
    )]
    InvalidDirectBootBinary,
}

#[derive(Error, Debug, Diagnostic)]
#[non_exhaustive]
pub enum ConnectionError {
    #[error("IO error while using serial port: {0}")]
    #[diagnostic(code(espflash::serial_error))]
    Serial(#[source] serial::core::Error),
    #[error("Failed to connect to the device")]
    #[diagnostic(
        code(espflash::connection_failed),
        help("Ensure that the device is connected and the reset and boot pins are not being held down")
    )]
    ConnectionFailed,
    #[error("Serial port not found")]
    #[diagnostic(
        code(espflash::connection_failed),
        help("Ensure that the device is connected and your host recognizes the serial adapter")
    )]
    DeviceNotFound,
    #[error("Timeout while running {0}command")]
    #[diagnostic(code(espflash::timeout))]
    Timeout(TimedOutCommand),
    #[error("Received packet has invalid SLIP framing")]
    #[diagnostic(
        code(espflash::slip_framing),
        help("Try hard-resetting the device and try again, if the error persists your rom might be corrupted")
    )]
    FramingError,
    #[error("Received packet to large for buffer")]
    #[diagnostic(
        code(espflash::oversized_packet),
        help("Try hard-resetting the device and try again, if the error persists your rom might be corrupted")
    )]
    OverSizedPacket,
}

#[derive(Debug, Default, Clone)]
pub struct TimedOutCommand {
    command: Option<Command>,
}

impl From<Command> for TimedOutCommand {
    fn from(c: Command) -> Self {
        TimedOutCommand { command: Some(c) }
    }
}

impl Display for TimedOutCommand {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.command {
            Some(command) => write!(f, "{} ", command),
            None => Ok(()),
        }
    }
}

impl From<serial::Error> for ConnectionError {
    fn from(err: serial::Error) -> Self {
        match err.kind() {
            serial::ErrorKind::Io(kind) => from_error_kind(kind, err),
            serial::ErrorKind::NoDevice => ConnectionError::DeviceNotFound,
            _ => ConnectionError::Serial(err),
        }
    }
}

impl From<serial::Error> for Error {
    fn from(err: serial::Error) -> Self {
        Self::Connection(err.into())
    }
}

impl From<io::Error> for ConnectionError {
    fn from(err: io::Error) -> Self {
        from_error_kind(err.kind(), err)
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Self::Connection(err.into())
    }
}

fn from_error_kind<E: Into<serial::Error>>(kind: io::ErrorKind, err: E) -> ConnectionError {
    match kind {
        io::ErrorKind::TimedOut => ConnectionError::Timeout(TimedOutCommand::default()),
        io::ErrorKind::NotFound => ConnectionError::DeviceNotFound,
        _ => ConnectionError::Serial(err.into()),
    }
}

impl From<SlipError> for ConnectionError {
    fn from(err: SlipError) -> Self {
        match err {
            SlipError::FramingError => Self::FramingError,
            SlipError::OversizedPacket => Self::OverSizedPacket,
            SlipError::ReadError(io) => Self::from(io),
            SlipError::EndOfStream => Self::FramingError,
        }
    }
}

impl From<SlipError> for Error {
    fn from(err: SlipError) -> Self {
        Self::Connection(err.into())
    }
}

impl From<binread::Error> for ConnectionError {
    fn from(err: binread::Error) -> Self {
        match err {
            binread::Error::Io(e) => ConnectionError::from(e),
            _ => unreachable!(),
        }
    }
}

impl From<binread::Error> for Error {
    fn from(err: binread::Error) -> Self {
        Self::Connection(err.into())
    }
}

#[derive(Copy, Clone, Debug, Error, Diagnostic)]
#[allow(dead_code)]
#[repr(u8)]
#[non_exhaustive]
pub enum RomErrorKind {
    #[error("Invalid message received")]
    #[diagnostic(code(espflash::rom::invalid_message))]
    InvalidMessage = 0x05,
    #[error("Bootloader failed to execute command")]
    #[diagnostic(code(espflash::rom::failed))]
    FailedToAct = 0x06,
    #[error("Received message has invalid crc")]
    #[diagnostic(code(espflash::rom::crc))]
    InvalidCrc = 0x07,
    #[error("Bootloader failed to write to flash")]
    #[diagnostic(code(espflash::rom::flash_write))]
    FlashWriteError = 0x08,
    #[error("Bootloader failed to read from flash")]
    #[diagnostic(code(espflash::rom::flash_read))]
    FlashReadError = 0x09,
    #[error("Invalid length for flash read")]
    #[diagnostic(code(espflash::rom::flash_read_length))]
    FlashReadLengthError = 0x0a,
    #[error("Malformed compressed data received")]
    #[diagnostic(code(espflash::rom::deflate))]
    DeflateError = 0x0b,
    #[error("Other")]
    #[diagnostic(code(espflash::rom::other))]
    Other = 0xff,
}

impl From<u8> for RomErrorKind {
    fn from(raw: u8) -> Self {
        match raw {
            0x05 => RomErrorKind::InvalidMessage,
            0x06 => RomErrorKind::FailedToAct,
            0x07 => RomErrorKind::InvalidCrc,
            0x08 => RomErrorKind::FlashWriteError,
            0x09 => RomErrorKind::FlashReadError,
            0x0a => RomErrorKind::FlashReadLengthError,
            0x0b => RomErrorKind::DeflateError,
            _ => RomErrorKind::Other,
        }
    }
}

#[derive(Copy, Clone, Debug, Error, Diagnostic)]
#[allow(dead_code)]
#[non_exhaustive]
#[error("Error while running {command} command")]
pub struct RomError {
    command: Command,
    #[source]
    kind: RomErrorKind,
}

impl RomError {
    pub fn new(command: Command, kind: RomErrorKind) -> RomError {
        RomError { command, kind }
    }
}

pub(crate) trait ResultExt {
    /// mark an error as having occurred during the flashing stage
    fn flashing(self) -> Self;
    /// mark the command from which this error originates
    fn for_command(self, command: Command) -> Self;
}

impl<T> ResultExt for Result<T, Error> {
    fn flashing(self) -> Self {
        match self {
            Err(Error::Connection(err)) => Err(Error::Flashing(err)),
            res => res,
        }
    }

    fn for_command(self, command: Command) -> Self {
        match self {
            Err(Error::Connection(ConnectionError::Timeout(_))) => {
                Err(Error::Connection(ConnectionError::Timeout(command.into())))
            }
            Err(Error::Flashing(ConnectionError::Timeout(_))) => {
                Err(Error::Flashing(ConnectionError::Timeout(command.into())))
            }
            res => res,
        }
    }
}

#[derive(Debug, Error, Diagnostic)]
pub enum PartitionTableError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Csv(#[from] CSVError),
    #[error(transparent)]
    #[diagnostic(transparent)]
    Overlapping(#[from] OverlappingPartitionsError),
    #[error(transparent)]
    #[diagnostic(transparent)]
    Duplicate(#[from] DuplicatePartitionsError),
    #[error(transparent)]
    #[diagnostic(transparent)]
    InvalidSubType(#[from] InvalidSubTypeError),
    #[error(transparent)]
    #[diagnostic(transparent)]
    UnalignedPartitionError(#[from] UnalignedPartitionError),
}

#[derive(Debug, Error, Diagnostic)]
#[error("Malformed partition table")]
#[diagnostic(
    code(espflash::partition_table::mallformed),
    help("{}See the espressif documentation for information on the partition table format:

https://docs.espressif.com/projects/esp-idf/en/latest/esp32/api-guides/partition-tables.html#creating-custom-tables", self.help)
)]
pub struct CSVError {
    #[source_code]
    source: String,
    #[label("{}", self.hint)]
    err_span: SourceSpan,
    hint: String,
    #[source]
    error: csv::Error,
    help: String,
}

impl CSVError {
    pub fn new(error: csv::Error, source: String) -> Self {
        let err_line = match error.kind() {
            csv::ErrorKind::Deserialize { pos: Some(pos), .. } => pos.line(),
            csv::ErrorKind::UnequalLengths { pos: Some(pos), .. } => pos.line(),
            _ => 0,
        };
        let mut hint = match error.kind() {
            csv::ErrorKind::Deserialize { err, .. } => err.to_string(),
            csv::ErrorKind::UnequalLengths {
                expected_len, len, ..
            } => format!(
                "record has {} fields, but the previous record has {} fields",
                len, expected_len
            ),
            _ => String::new(),
        };
        let mut help = String::new();

        // string matching is fragile but afaik there is no better way in this case
        // and if it does break the error is still not bad
        if hint == "data did not match any variant of untagged enum SubType" {
            hint = "Unknown sub-type".into();
            help = format!(
                "the following sub-types are supported:
    {} for data partitions
    {} for app partitions\n\n",
                Type::Data.subtype_hint(),
                Type::App.subtype_hint()
            )
        }

        let err_span = line_to_span(&source, err_line as usize);

        CSVError {
            source,
            err_span,
            hint,
            error,
            help,
        }
    }
}

/// since csv doesn't give us the position in the line the error occurs, we highlight the entire line
///
/// line starts at 1
fn line_to_span(source: &str, line: usize) -> SourceSpan {
    let line_length = source.lines().nth(line - 1).unwrap().len().into();
    SourceSpan::new(SourceOffset::from_location(source, line, 2), line_length)
}

#[derive(Debug, Error, Diagnostic)]
#[error("Overlapping partitions")]
#[diagnostic(code(espflash::partition_table::overlapping))]
pub struct OverlappingPartitionsError {
    #[source_code]
    source_code: String,
    #[label("This partition")]
    partition1_span: SourceSpan,
    #[label("overlaps with this partition")]
    partition2_span: SourceSpan,
}

impl OverlappingPartitionsError {
    pub fn new(source: &str, partition1_line: usize, partition2_line: usize) -> Self {
        OverlappingPartitionsError {
            source_code: source.into(),
            partition1_span: line_to_span(source, partition1_line),
            partition2_span: line_to_span(source, partition2_line),
        }
    }
}

#[derive(Debug, Error, Diagnostic)]
#[error("Duplicate partitions")]
#[diagnostic(code(espflash::partition_table::duplicate))]
pub struct DuplicatePartitionsError {
    #[source_code]
    source_code: String,
    #[label("This partition")]
    partition1_span: SourceSpan,
    #[label("has the same {} as this partition", self.ty)]
    partition2_span: SourceSpan,
    ty: &'static str,
}

impl DuplicatePartitionsError {
    pub fn new(
        source: &str,
        partition1_line: usize,
        partition2_line: usize,
        ty: &'static str,
    ) -> Self {
        DuplicatePartitionsError {
            source_code: source.into(),
            partition1_span: line_to_span(source, partition1_line),
            partition2_span: line_to_span(source, partition2_line),
            ty,
        }
    }
}

#[derive(Debug, Error, Diagnostic)]
#[error("Invalid subtype for type")]
#[diagnostic(
    code(espflash::partition_table::invalid_type),
    help("'{}' supports the following subtypes: {}", self.ty, self.ty.subtype_hint())
)]
pub struct InvalidSubTypeError {
    #[source_code]
    source_code: String,
    #[label("'{}' is not a valid subtype for '{}'", self.sub_type, self.ty)]
    span: SourceSpan,
    ty: Type,
    sub_type: SubType,
}

impl InvalidSubTypeError {
    pub fn new(source: &str, line: usize, ty: Type, sub_type: SubType) -> Self {
        InvalidSubTypeError {
            source_code: source.into(),
            span: line_to_span(source, line),
            ty,
            sub_type,
        }
    }
}

#[derive(Debug, Error, Diagnostic)]
#[error("Unaligned partition")]
#[diagnostic(code(espflash::partition_table::unaligned))]
pub struct UnalignedPartitionError {
    #[source_code]
    source_code: String,
    #[label("App partition is not aligned to 64k (0x10000)")]
    span: SourceSpan,
}

impl UnalignedPartitionError {
    pub fn new(source: &str, line: usize) -> Self {
        UnalignedPartitionError {
            source_code: source.into(),
            span: line_to_span(source, line),
        }
    }
}

#[derive(Debug, Error)]
#[error("{0}")]
pub struct ElfError(&'static str);

impl From<&'static str> for ElfError {
    fn from(err: &'static str) -> Self {
        ElfError(err)
    }
}

#[derive(Debug, Error)]
#[error("Unrecognized magic value {0:#x}")]
pub struct ChipDetectError(u32);

impl From<u32> for ChipDetectError {
    fn from(err: u32) -> Self {
        ChipDetectError(err)
    }
}

#[derive(Debug, Error)]
#[error("Unrecognized flash id {0:#x}")]
pub struct FlashDetectError(u8);

impl From<u8> for FlashDetectError {
    fn from(err: u8) -> Self {
        FlashDetectError(err)
    }
}

#[derive(Debug, Error, Diagnostic)]
#[error("Image format {format} is not supported by the {chip}")]
#[diagnostic(
    code(espflash::unsupported_image_format),
    help("The following image formats are supported by the {}: {}", self.chip, self.supported_formats())
)]
pub struct UnsupportedImageFormatError {
    format: ImageFormatId,
    chip: Chip,
}

impl UnsupportedImageFormatError {
    pub fn new(format: ImageFormatId, chip: Chip) -> Self {
        UnsupportedImageFormatError { format, chip }
    }

    fn supported_formats(&self) -> String {
        self.chip
            .supported_image_formats()
            .iter()
            .map(|format| format.as_static())
            .collect::<Vec<&'static str>>()
            .join(", ")
    }
}
