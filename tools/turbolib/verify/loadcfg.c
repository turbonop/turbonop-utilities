/* The fixture program's load configuration, which names the linker's
   /guard:cf tables, as the CRT's loadcfg.c does (in C: the guard flags'
   truncated address is a constant there). */
extern char __guard_fids_table[];
extern char __guard_fids_count[];
extern char __guard_flags[];
extern char __guard_iat_table[];
extern char __guard_iat_count[];
extern char __guard_longjmp_table[];
extern char __guard_longjmp_count[];
extern void* __guard_check_icall_fptr;
extern void* __guard_dispatch_icall_fptr;
/* What the C runtime would define: floating point's marker, and the
   security cookie. */
int _fltused = 0;
unsigned long long __security_cookie = 0x2B992DDFA232;

typedef struct LoadConfig {
    unsigned int Size, TimeDateStamp;
    unsigned short MajorVersion, MinorVersion;
    unsigned int GlobalFlagsClear, GlobalFlagsSet, CriticalSectionDefaultTimeout;
    unsigned long long DeCommitFreeBlockThreshold, DeCommitTotalFreeThreshold, LockPrefixTable,
        MaximumAllocationSize, VirtualMemoryThreshold, ProcessAffinityMask;
    unsigned int ProcessHeapFlags;
    unsigned short CSDVersion, DependentLoadFlags;
    unsigned long long EditList, SecurityCookie, SEHandlerTable, SEHandlerCount,
        GuardCFCheckFunctionPointer, GuardCFDispatchFunctionPointer, GuardCFFunctionTable,
        GuardCFFunctionCount;
    unsigned int GuardFlags;
    unsigned char CodeIntegrity[12];
    unsigned long long GuardAddressTakenIatEntryTable, GuardAddressTakenIatEntryCount,
        GuardLongJumpTargetTable, GuardLongJumpTargetCount;
} LoadConfig;
const LoadConfig _load_config_used = {
    sizeof(LoadConfig), 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    (unsigned long long)&__security_cookie, 0, 0,
    (unsigned long long)&__guard_check_icall_fptr,
    (unsigned long long)&__guard_dispatch_icall_fptr,
    (unsigned long long)&__guard_fids_table,
    (unsigned long long)&__guard_fids_count,
    (unsigned int)(unsigned long long)&__guard_flags, {0},
    (unsigned long long)&__guard_iat_table,
    (unsigned long long)&__guard_iat_count,
    (unsigned long long)&__guard_longjmp_table,
    (unsigned long long)&__guard_longjmp_count,
};

unsigned int load_config_size(void) { return _load_config_used.Size; }
