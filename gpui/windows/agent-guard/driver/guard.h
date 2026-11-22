#pragma once

#include <fltKernel.h>
#include "protocol.h"

_IRQL_requires_max_(DISPATCH_LEVEL)
BOOLEAN GuardIsEnrolled(_In_opt_ PEPROCESS Process);
VOID GuardCheckPolicyContract(VOID);

extern const FLT_OPERATION_REGISTRATION GuardOperations[];
