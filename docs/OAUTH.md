# Apps OAuth (Microsoft e Google)

Quem usa o Linha só clica em **Conectar** nos Ajustes: o navegador abre, a pessoa entra na conta e autoriza. Para isso funcionar, o executável já sai do build com o ID de um app OAuth do Linha para cada serviço. Esses apps são registrados **uma vez**, por quem mantém o projeto.

Os IDs entram no build por variáveis de ambiente (lidas com `option_env!` em `src/config.rs`):

| variável                      | de onde vem                                  |
| ----------------------------- | -------------------------------------------- |
| `LINHA_OUTLOOK_CLIENT_ID`     | Microsoft Entra → ID do aplicativo (cliente) |
| `LINHA_GOOGLE_CLIENT_ID`      | Google Cloud → ID do cliente OAuth           |
| `LINHA_GOOGLE_CLIENT_SECRET`  | Google Cloud → chave secreta do cliente      |

Sem elas o app compila normalmente, mas o botão **Conectar** daquele serviço fica desativado.

> O "client secret" do Google, para apps de desktop, **não é secreto**: qualquer um pode extraí-lo do executável, e o Google sabe disso. A proteção real vem do PKCE e do redirecionamento para `127.0.0.1`. Mesmo assim, mantenha-o fora do repositório (use secrets do GitHub).

## Microsoft (Outlook / Microsoft 365)

1. https://entra.microsoft.com → **Aplicativos → Registros de aplicativo → Novo registro**.
2. Nome: `Linha`. Tipos de conta: **contas em qualquer diretório organizacional e contas Microsoft pessoais**.
3. URI de redirecionamento: plataforma **Cliente público/nativo (móvel e área de trabalho)**, valor `http://localhost`.
   (A Microsoft aceita qualquer porta em `http://localhost`; o Linha escolhe uma livre a cada login.)
4. Em **Permissões de API**, confira *Microsoft Graph → Delegada → `Calendars.Read`* e `offline_access`.
5. Copie o **ID do aplicativo (cliente)** → `LINHA_OUTLOOK_CLIENT_ID`.

Não é preciso criar segredo de cliente: o app é público e usa PKCE.

> Em contas corporativas, o administrador pode exigir aprovação do app. Para publicar para qualquer pessoa sem avisos, faça a [verificação de editor](https://learn.microsoft.com/entra/identity-platform/publisher-verification-overview).

## Google (Google Agenda / Gmail)

1. https://console.cloud.google.com → crie um projeto e ative a **Google Calendar API**.
2. **Tela de consentimento OAuth**: tipo *Externo*, escopo `.../auth/calendar.readonly`.
3. **Credenciais → Criar credenciais → ID do cliente OAuth → App para computador**.
4. Copie o ID e a chave secreta → `LINHA_GOOGLE_CLIENT_ID` e `LINHA_GOOGLE_CLIENT_SECRET`.

> Enquanto o app estiver em modo **Teste**, só os e-mails cadastrados como usuários de teste conseguem entrar, e o login expira a cada 7 dias. Para liberar para todos, publique o app. Como `calendar.readonly` é um escopo sensível, o Google pede uma verificação (política de privacidade, domínio e um vídeo do fluxo de login).

## iCloud

A Apple não oferece OAuth para acessar agendas de terceiros. Nos Ajustes, a pessoa informa o Apple ID e uma **senha de app** (gerada em account.apple.com → Entrar e segurança → Senhas de app), que o Linha usa no CalDAV.

## Configurar no GitHub

Em **Settings → Secrets and variables → Actions → New repository secret**, crie os três secrets da tabela acima. O workflow de release (`.github/workflows/release.yml`) os repassa para o `cargo build`.

## Desenvolvimento local

```powershell
# PowerShell
$env:LINHA_OUTLOOK_CLIENT_ID = "..."
$env:LINHA_GOOGLE_CLIENT_ID = "..."
$env:LINHA_GOOGLE_CLIENT_SECRET = "..."
cargo run -- ajustes
```

```bash
# bash
export LINHA_OUTLOOK_CLIENT_ID=... LINHA_GOOGLE_CLIENT_ID=... LINHA_GOOGLE_CLIENT_SECRET=...
cargo run -- ajustes
```

Também dá para testar com apps próprios sem recompilar: preencha `client_id` (e `client_secret`, no Google) nas seções `[outlook]` / `[google]` do `config.toml`. Esses campos, quando preenchidos, têm prioridade sobre os embutidos.
