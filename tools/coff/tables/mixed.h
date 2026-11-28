/* COMDATs that a C and a C++ translation unit both define, as UCRT's C
   and C++ sources share _Fenv1 and floating-point constants: cl's C and
   C++ front ends write the same bytes, under their own @comp.id and
   S_COMPILE3, so turbolib keeps the two languages' copies apart. */
#pragma once
#ifdef __cplusplus
extern "C" {
#endif

/* A selectany constant (pick any), as UCRT's _Fenv1. */
__declspec(selectany) extern const unsigned long long mixed_env = 0x3f00000000000000ull;

#ifdef __cplusplus
}
#endif
