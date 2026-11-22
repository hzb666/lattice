#pragma once

#define GUARD_PORT_NAME L"\\LatticeAgentGuard"
#define GUARD_PROTOCOL_VERSION 2u
#define GUARD_OP_PROBE 0u
#define GUARD_OP_ENROLL 1u
#define GUARD_OP_VOLUME_READY 2u

#pragma pack(push, 4)
typedef struct _GUARD_REQUEST {
    UINT32 Version;
    UINT32 Operation;
    UINT64 Handle;
} GUARD_REQUEST;
#pragma pack(pop)

C_ASSERT(sizeof(GUARD_REQUEST) == 16);
C_ASSERT(TYPE_ALIGNMENT(GUARD_REQUEST) == 4);
C_ASSERT(FIELD_OFFSET(GUARD_REQUEST, Operation) == 4);
C_ASSERT(FIELD_OFFSET(GUARD_REQUEST, Handle) == 8);
