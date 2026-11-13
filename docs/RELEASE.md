# Como lançar uma nova versão

As releases são geradas pelo GitHub Actions (`.github/workflows/release.yml`). Você só precisa atualizar a versão e enviar uma tag: o pipeline compila para Linux, Windows e macOS e publica a release com os binários.

## Versionamento

Usamos [SemVer](https://semver.org/lang/pt-BR/): `MAJOR.MINOR.PATCH`.

| Mudança                                                 | Exemplo           |
| ------------------------------------------------------- | ----------------- |
| Correção de bug, sem mudar comportamento                | `0.1.0 → 0.1.1`   |
| Funcionalidade nova, compatível com a config existente  | `0.1.1 → 0.2.0`   |
| Quebra de compatibilidade (config, comandos, etc.)      | `0.2.0 → 1.0.0`   |
| Pré-release (sai marcada como *pre-release* no GitHub)  | `0.2.0-beta.1`    |

## Passo a passo

1. **Comece da `main` atualizada e sem alterações pendentes**

   ```bash
   git checkout main
   git pull
   git status        # deve estar limpo
   ```

2. **Atualize a versão no `Cargo.toml`**

   ```toml
   [package]
   version = "0.2.0"
   ```

   Depois rode o build para atualizar o `Cargo.lock` e confira os testes:

   ```bash
   cargo build --release
   cargo test
   ```

3. **Faça o commit da versão**

   ```bash
   git add Cargo.toml Cargo.lock
   git commit -m "release: v0.2.0"
   git push origin main
   ```

4. **Crie e envie a tag** (precisa ter o `v` na frente e ser igual à versão do `Cargo.toml`)

   ```bash
   git tag -a v0.2.0 -m "v0.2.0"
   git push origin v0.2.0
   ```

5. **Acompanhe o pipeline** em *Actions → Release*. Em ~10 min a release aparece em *Releases* com:

   - `linha-v0.2.0-x86_64-unknown-linux-gnu.tar.gz`
   - `linha-v0.2.0-x86_64-pc-windows-msvc.zip`
   - `linha-v0.2.0-aarch64-apple-darwin.tar.gz` (Mac Apple Silicon)
   - `linha-v0.2.0-x86_64-apple-darwin.tar.gz` (Mac Intel)
   - um `.sha256` para cada arquivo

   As notas da release são geradas automaticamente a partir dos commits/PRs. Dá para editá-las depois na página da release.

## Rodar manualmente

Em *Actions → Release → Run workflow*, informe a tag (ex.: `v0.2.0`). A versão do `Cargo.toml` no branch escolhido precisa bater com a tag; se a tag não existir, ela é criada no commit atual.

## Quando algo dá errado

**"A tag vX não bate com a versão do Cargo.toml"** — você esqueceu o passo 2 ou digitou a tag diferente. Apague a tag e refaça:

```bash
git tag -d v0.2.0
git push origin :refs/tags/v0.2.0
# corrija o Cargo.toml, faça commit e volte ao passo 4
```

**Falhou o build de uma plataforma** — corrija o código, faça commit na `main` e mova a tag para o novo commit:

```bash
git tag -d v0.2.0
git push origin :refs/tags/v0.2.0
git tag -a v0.2.0 -m "v0.2.0"
git push origin v0.2.0
```

Se a release já tiver sido criada pela metade, apague-a em *Releases* antes de reenviar a tag.

**Erro de permissão ao criar a release** — em *Settings → Actions → General → Workflow permissions*, marque **Read and write permissions**.

## Observações

- Os binários de macOS não são assinados: na primeira vez, abra com botão direito → *Abrir*, ou rode `xattr -d com.apple.quarantine ./linha`.
- No Windows o SmartScreen pode avisar por ser um executável não assinado: *Mais informações → Executar assim mesmo*.
