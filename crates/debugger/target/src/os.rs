//! The operating system a dump came from.

use turbonop_abi::Os;
use turbonop_minidump::platform;

/// The operating system a minidump's `PlatformId` names (Windows', or
/// Breakpad's for the others), if it's one whose conventions are known.
pub fn of_minidump(platform_id: u32) -> Option<Os> {
    Some(match platform_id {
        // (Windows' other values: Win32s, Windows 9x, and CE.)
        platform::WIN32_NT => Os::Windows,
        platform::LINUX => Os::Linux,
        platform::ANDROID => Os::Android,
        platform::MAC_OS_X => Os::MacOs,
        platform::IOS => Os::Ios,
        _ => return None,
    })
}
