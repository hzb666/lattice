import { describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import {
  executeAgentProjectDocumentToolRequest,
  parseAgentProjectDocumentToolRequest,
  replaceAgentProjectText,
  SYNARA_PROJECT_DOCUMENT_TOOL_REQUEST,
  type AgentProjectDocumentToolRequest,
} from "./agent-project-document-tools";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

function request(
  path: string,
  documentType: "board" | "spreadsheet",
): AgentProjectDocumentToolRequest {
  return {
    type: SYNARA_PROJECT_DOCUMENT_TOOL_REQUEST,
    version: 1,
    id: "request-1",
    args: { path, documentType },
    expiresAt: Date.now() + 1_000,
  };
}

describe("agent project document host protocol", () => {
  it("strictly parses normalized board and spreadsheet requests", () => {
    const board = request("figures/model.tldr", "board");
    const spreadsheet = request("data/results.lattice-sheet", "spreadsheet");
    expect(parseAgentProjectDocumentToolRequest(board)).toEqual(board);
    expect(parseAgentProjectDocumentToolRequest(spreadsheet)).toEqual(spreadsheet);
    expect(parseAgentProjectDocumentToolRequest({ ...board, version: 2 })).toBeNull();
    expect(parseAgentProjectDocumentToolRequest({ ...board, unexpected: true })).toBeNull();
    expect(parseAgentProjectDocumentToolRequest(request("../model.tldr", "board"))).toBeNull();
    expect(parseAgentProjectDocumentToolRequest(request("model.lattice-sheet", "board"))).toBeNull();
    expect(parseAgentProjectDocumentToolRequest(request("model.tldr", "spreadsheet"))).toBeNull();
  });

  it("creates and reports an opened document through the registered host callback", async () => {
    const createDocument = vi.fn(async (input: AgentProjectDocumentToolRequest) => input.args.path);
    await expect(executeAgentProjectDocumentToolRequest(
      request("results.lattice-sheet", "spreadsheet"),
      createDocument,
    )).resolves.toMatchObject({
      ok: true,
      result: {
        path: "results.lattice-sheet",
        documentType: "spreadsheet",
        opened: true,
      },
    });
    expect(createDocument).toHaveBeenCalledOnce();
  });

  it("rejects expired requests and a missing host without creating anything", async () => {
    const expired = request("model.tldr", "board");
    expired.expiresAt = Date.now() - 1;
    const createDocument = vi.fn(async () => "model.tldr");
    await expect(executeAgentProjectDocumentToolRequest(expired, createDocument)).resolves.toMatchObject({
      ok: false,
      error: { code: "project_document_tool_expired" },
    });
    expect(createDocument).not.toHaveBeenCalled();
    await expect(executeAgentProjectDocumentToolRequest(
      request("model.tldr", "board"),
      null,
    )).resolves.toMatchObject({
      ok: false,
      error: { code: "project_document_host_unavailable" },
    });
  });

  it("routes an isolated manuscript edit to the native host and reports the save", async () => {
    const envelope = {
      ...request("unused.tldr", "board"),
      args: {
        path: "main.tex", documentType: "tex", oldText: "draft", newText: "revision",
        threadId: "thread-1", turnId: "turn-1",
      },
    };
    await expect(replaceAgentProjectText(envelope, null, "stage")).resolves.toMatchObject({ ok: false });
    await expect(replaceAgentProjectText(envelope, "project", null)).resolves.toMatchObject({ ok: false });
    await expect(replaceAgentProjectText({
      ...envelope,
      args: { ...envelope.args, path: "references.bib" },
    }, "project", "stage")).resolves.toMatchObject({ ok: false });
    expect(invoke).not.toHaveBeenCalled();

    await expect(replaceAgentProjectText(envelope, "project", "stage")).resolves.toMatchObject({
      ok: true,
      result: { path: "main.tex", saved: true },
    });
    expect(invoke).toHaveBeenCalledExactlyOnceWith("agent_replace_project_text", {
      projectRoot: "project",
      edit: {
        path: "main.tex", oldText: "draft", newText: "revision",
        threadId: "thread-1", turnId: "turn-1", expiresAt: envelope.expiresAt,
      },
    });
  });
});
