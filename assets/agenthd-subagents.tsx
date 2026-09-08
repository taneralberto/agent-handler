/** @jsxImportSource @opentui/solid */
import { createResource, createSignal, type JSX } from "@opentui/solid"
import type { TuiPlugin, TuiPluginApi, TuiPluginModule, TuiSlotPlugin } from "@opencode-ai/plugin/tui"

type Session = {
  id: string
  parentID?: string
  title: string
  agent?: string
  model?: { providerID: string; modelID: string }
  tokens: { input: number; output: number }
}

type TreeProps = {
  api: TuiPluginApi
  sessions: Session[]
  parentID: string
  depth: number
}

const model = (session: Session) =>
  session.model ? `${session.model.providerID}/${session.model.modelID}` : "(inherit)"

const tokens = (session: Session) => `${Math.round(session.tokens.input / 1000)}k ctx`

const Tree = (props: TreeProps): JSX.Element => {
  const children = props.sessions.filter((session) => session.parentID === props.parentID)
  return (
    <box flexDirection="column">
      {children.map((session) => {
        const status = props.api.state.session.status(session.id)?.type ?? "idle"
        return (
          <box flexDirection="column">
            <text fg={props.api.theme.current.text}>
              {"  ".repeat(props.depth)}<span style={{ fg: props.api.theme.current.primary }}>●</span> {session.title}
            </text>
            <text fg={props.api.theme.current.textMuted}>
              {"  ".repeat(props.depth + 1)}{session.agent ?? "agent"} · {model(session)} · {tokens(session)} · {status}
            </text>
            <Tree api={props.api} sessions={props.sessions} parentID={session.id} depth={props.depth + 1} />
          </box>
        )
      })}
    </box>
  )
}

const Panel = (props: { api: TuiPluginApi; sessionID: string }) => {
  const [version, setVersion] = createSignal(0)
  const refresh = () => setVersion((value) => value + 1)
  const [sessions] = createResource(version, async () => {
    const result = await props.api.client.session.list({ directory: props.api.state.path.directory, limit: 100 })
    return result.data?.data ?? []
  })

  for (const event of ["session.created", "session.updated", "session.status"] as const) {
    props.api.lifecycle.onDispose(props.api.event.on(event, refresh))
  }

  const list = () => sessions() ?? []
  const hasTasks = () => list().some((session) => session.parentID === props.sessionID)

  return (
    <box border borderColor={props.api.theme.current.border} flexDirection="column" gap={1} paddingLeft={1} paddingRight={1}>
      <box flexDirection="row" justifyContent="space-between">
        <text fg={props.api.theme.current.primary}><b>Subagents</b></text>
        <text fg={props.api.theme.current.textMuted}>tasks · model · context</text>
      </box>
      {sessions.loading ? <text fg={props.api.theme.current.textMuted}>Loading tasks…</text> : null}
      {sessions.error ? <text fg={props.api.theme.current.error}>Unable to load subagents.</text> : null}
      {!sessions.loading && !sessions.error && !hasTasks() ? (
        <text fg={props.api.theme.current.textMuted}>No subagent tasks for this session.</text>
      ) : null}
      {!sessions.loading && !sessions.error ? <Tree api={props.api} sessions={list()} parentID={props.sessionID} depth={0} /> : null}
    </box>
  )
}

const tui: TuiPlugin = async (api) => {
  const panel: TuiSlotPlugin = {
    order: 400,
    slots: {
      sidebar_content(_context, value) {
        return <Panel api={api} sessionID={value.session_id} />
      },
    },
  }
  api.slots.register(panel)
}

const plugin: TuiPluginModule & { id: string } = {
  id: "agenthd-subagents",
  tui,
}

export default plugin
