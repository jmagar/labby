# Dozzle

Direct Dozzle API workflow for the real-time Docker container log viewer. Uses
`DOZZLE_URL` and optional `DOZZLE_SESSION_COOKIE`; covers Dozzle auth/MCP
guidance; configures Dozzle's native Streamable HTTP MCP endpoint at `/api/mcp`;
does not route through the stale Lab MCP wrapper or `lab dozzle`.
