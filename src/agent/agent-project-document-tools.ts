/* eslint-disable lingui/no-unlocalized-strings -- Protocol field names and Agent diagnostics are never rendered by Lattice UI. */

import { invoke } from "@tauri-apps/api/core";
import { isRecord } from "../app-utils";
import {
  hasOnlyKeys,
  isWorkspaceRelativePath,
  parseToolEnvelope,
  runAgentTool,
  toolError,
  type AgentToolResult,
} from "./agent-protocol";

export const SYNARA_PROJECT_DOCUMENT_TOOL_REQUEST = "synara:project-document-tool-request";
const LATTICE_PROJECT_DOCUMENT_TOOL_RESULT = "lattice:project-document-tool-result";

type AgentProjectDocumentType = "board" | "spreadsheet";

const EXTENSIONS: Record<AgentProjectDocumentType, string> = { board: ".excalidraw", spreadsheet: ".lattice-sheet" };
const LEGACY_BOARD_EXTENSION = ".tldr";

export type AgentProjectDocumentToolRequest = {
  type: typeof SYNARA_PROJECT_DOCUMENT_TOOL_REQUEST;
  version: 1;
  id: string;
  args: {
    path: string;
    documentType: AgentProjectDocumentType;
  };
  expiresAt: number;
};

export type AgentProjectDocumentToolResult = AgentToolResult<
  typeof LATTICE_PROJECT_DOCUMENT_TOOL_RESULT,
  { path: string; documentType: AgentProjectDocumentType; opened: true }
>;

type AgentProjectDocumentCreator = (
  request: AgentProjectDocumentToolRequest,
) => Promise<string>;

export function parseAgentProjectDocumentToolRequest(
  value: unknown,
): AgentProjectDocumentToolRequest | null {
  const request = parseToolEnvelope(value, SYNARA_PROJECT_DOCUMENT_TOOL_REQUEST, ["args"]);
  if (!request || !isRecord(request.args) || !hasOnlyKeys(request.args, ["path", "documentType"])) return null;
  const { documentType } = request.args;
  let { path } = request.args;
  if (!isWorkspaceRelativePath(path) || (documentType !== "board" && documentType !== "spreadsheet")) return null;
  const lower = path.toLocaleLowerCase("en-US");
  if (documentType === "board" && lower.endsWith(LEGACY_BOARD_EXTENSION)) {
    path = `${path.slice(0, -LEGACY_BOARD_EXTENSION.length)}${EXTENSIONS.board}`;
  } else if (!lower.endsWith(EXTENSIONS[documentType])) {
    return null;
  }
  return { ...request, args: { path, documentType } } as AgentProjectDocumentToolRequest;
}

export function rejectAgentProjectDocumentToolRequest(value: unknown): Promise<AgentProjectDocumentToolResult> | null {
  const envelope = parseToolEnvelope(value, SYNARA_PROJECT_DOCUMENT_TOOL_REQUEST);
  if (!envelope) return null;
  return runAgentTool(LATTICE_PROJECT_DOCUMENT_TOOL_RESULT, envelope.id as string, "project_document_invalid_arguments", () => {
    throw toolError(
      "path must be a normalized workspace-relative path ending in .excalidraw for a board or .lattice-sheet for a spreadsheet.",
      "project_document_invalid_arguments",
    );
  });
}

export function executeAgentProjectDocumentToolRequest(
  request: AgentProjectDocumentToolRequest,
  createDocument: AgentProjectDocumentCreator | null,
): Promise<AgentProjectDocumentToolResult> {
  return runAgentTool(LATTICE_PROJECT_DOCUMENT_TOOL_RESULT, request.id, "project_document_create_failed", async () => {
    if (request.expiresAt <= Date.now()) {
      throw toolError("The project document request expired before execution.", "project_document_tool_expired");
    }
    if (!createDocument) {
      throw toolError("The Lattice project document host is unavailable.", "project_document_host_unavailable");
    }
    const path = await createDocument(request);
    if (path !== request.args.path) {
      throw toolError("Lattice created the project document at an unexpected path.", "project_document_path_mismatch");
    }
    return { path, documentType: request.args.documentType, opened: true as const };
  });
}

export function replaceAgentProjectText(
  value: unknown,
  projectRoot: string | null,
  executionRoot: string | null,
): Promise<AgentToolResult<
  typeof LATTICE_PROJECT_DOCUMENT_TOOL_RESULT,
  { path: string; saved: true } | { conflictedPaths: string[] } | { paths: string[] }
>> | null {
  const request = parseToolEnvelope(value, SYNARA_PROJECT_DOCUMENT_TOOL_REQUEST, ["args"]);
  const args = request?.args;
  if (!request || !isRecord(args)
    || (args.documentType !== "tex" && args.documentType !== "texUndo" && args.documentType !== "texHistory")) return null;
  return runAgentTool(LATTICE_PROJECT_DOCUMENT_TOOL_RESULT, request.id as string, "manuscript_edit_failed", async () => {
    if (!projectRoot || !executionRoot || (request.expiresAt as number) <= Date.now()) {
      throw toolError("The isolated manuscript edit is unavailable or expired.", "manuscript_edit_unavailable");
    }
    const expiresAt = request.expiresAt;
    if (args.documentType === "texHistory") {
      const { threadId, turnId } = args;
      if (!hasOnlyKeys(args, ["documentType", "threadId", "turnId"]) || !isTurnId(threadId) || !isTurnId(turnId)) {
        throw toolError("Invalid manuscript history request.", "manuscript_edit_invalid");
      }
      const paths = await invoke<string[]>("agent_project_text_history", {
        projectRoot, edit: { threadId, turnId, expiresAt },
      });
      return { paths };
    }
    if (args.documentType === "texUndo") {
      const { threadId, turnIds, operationId, redo } = args;
      if (
        !hasOnlyKeys(args, ["documentType", "threadId", "turnIds", "operationId", "redo"])
        || !isTurnId(threadId) || !isTurnId(operationId) || !Array.isArray(turnIds)
        || turnIds.length === 0 || turnIds.length > 128 || !turnIds.every(isTurnId)
        || typeof redo !== "boolean"
      ) throw toolError("Invalid manuscript Undo request.", "manuscript_edit_invalid");
      const conflictedPaths = await invoke<string[]>("agent_undo_project_text", {
        projectRoot, edit: { threadId, turnIds, operationId, redo, expiresAt },
      });
      return { conflictedPaths };
    }
    const { path, oldText, newText, threadId, turnId } = args;
    if (
      !hasOnlyKeys(args, ["path", "documentType", "oldText", "newText", "threadId", "turnId"])
      || !isTurnId(threadId) || !isTurnId(turnId)
      || !isWorkspaceRelativePath(path)
      || !/\.(?:tex|md|sty|cls|txt)$/i.test(path)
      || typeof oldText !== "string" || oldText.length === 0 || oldText.length > 16_384
      || typeof newText !== "string" || newText.length > 16_384
    ) {
      throw toolError("Invalid manuscript replacement.", "manuscript_edit_invalid");
    }
    await invoke("agent_replace_project_text", {
      projectRoot,
      edit: { path, oldText, newText, threadId, turnId, expiresAt },
    });
    return { path, saved: true as const };
  });
}

function isTurnId(value: unknown): value is string {
  return typeof value === "string" && value.length > 0 && value.length <= 256 && !value.includes("\0");
}
