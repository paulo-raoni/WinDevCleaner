//! Message catalogue: every user-facing message in English and Brazilian Portuguese.
//!
//! The catalogue is a table built by a macro, so a message cannot exist in one language only:
//! each entry must give both texts or it does not compile. Placeholders are written `{name}`
//! and filled by [`Lang::fmt`]. JSON output for agents does not go through this catalogue; it
//! stays English and stable (plan, section 9 item 2).

/// Interface language.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    En,
    PtBr,
}

impl Lang {
    /// Parses a `--lang` value: `en`, `pt-BR` (also `pt`, `pt_br`), case-insensitive.
    pub fn parse(s: &str) -> Option<Lang> {
        match s.to_ascii_lowercase().replace('_', "-").as_str() {
            "en" | "en-us" | "en-gb" | "english" => Some(Lang::En),
            "pt" | "pt-br" | "portugues" | "português" => Some(Lang::PtBr),
            _ => None,
        }
    }

    /// The language tag shown to humans and accepted by `--lang`.
    pub fn tag(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::PtBr => "pt-BR",
        }
    }

    /// Follows the Windows display language: Portuguese uses pt-BR, everything else English.
    pub fn detect() -> Lang {
        if let Ok(v) = std::env::var("WDC_LANG") {
            if let Some(l) = Lang::parse(&v) {
                return l;
            }
        }
        #[cfg(windows)]
        {
            if crate::win::ui_language_is_portuguese() {
                return Lang::PtBr;
            }
            Lang::En
        }
        #[cfg(not(windows))]
        {
            let env = std::env::var("LANG").unwrap_or_default().to_ascii_lowercase();
            if env.starts_with("pt") {
                Lang::PtBr
            } else {
                Lang::En
            }
        }
    }

    /// The text of a message in this language.
    pub fn t(self, msg: Msg) -> &'static str {
        msg.text(self)
    }

    /// The text of a message with its `{name}` placeholders filled in.
    pub fn fmt(self, msg: Msg, args: &[(&str, &str)]) -> String {
        let mut out = msg.text(self).to_string();
        for (name, value) in args {
            out = out.replace(&format!("{{{name}}}"), value);
        }
        out
    }
}

macro_rules! messages {
    ($($key:ident => $en:literal, $pt:literal;)*) => {
        /// A user-facing message.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Msg { $($key),* }

        impl Msg {
            /// Every message, for completeness tests.
            pub const ALL: &'static [Msg] = &[$(Msg::$key),*];

            /// The text of this message in `lang`.
            pub fn text(self, lang: Lang) -> &'static str {
                match lang {
                    Lang::En => match self { $(Msg::$key => $en),* },
                    Lang::PtBr => match self { $(Msg::$key => $pt),* },
                }
            }
        }
    };
}

messages! {
    // Command line.
    CliUsage =>
        "WinDevCleaner {version}: find where your disk space is. Read-only in this version.\n\nUsage:\n  wdc scan [PATH ...] [--json] [--throttle MS] [--top N] [--depth N] [--lang en|pt-BR]\n      Gentle scan (background mode) of the read scope, or of the given folders.\n      Saves a snapshot in the WinDevCleaner data folder; changes nothing else.\n  wdc explain PATH [--json]\n      Why a folder or file is here, the rule that knows it, and what deleting it would free.\n  wdc diff [--from SNAPSHOT] [--to SNAPSHOT] [--min-mb N] [--limit N] [--json]\n      What grew since the previous scan of the same folders (from the saved snapshots).\n  wdc mcp [--show]\n      MCP server on stdio for AI agents. Drives the WinDevCleaner app when it is open\n      (--show opens it); otherwise runs headless.\n  wdc --version\n",
        "WinDevCleaner {version}: descubra onde está o espaço do seu disco. Somente leitura nesta versão.\n\nUso:\n  wdc scan [PASTA ...] [--json] [--throttle MS] [--top N] [--depth N] [--lang en|pt-BR]\n      Análise suave (modo em segundo plano) do escopo de leitura, ou das pastas indicadas.\n      Salva um snapshot na pasta de dados do WinDevCleaner; não altera mais nada.\n  wdc explain CAMINHO [--json]\n      Por que uma pasta ou arquivo está aqui, a regra que o conhece, e o que apagar liberaria.\n  wdc diff [--from SNAPSHOT] [--to SNAPSHOT] [--min-mb N] [--limit N] [--json]\n      O que cresceu desde a análise anterior das mesmas pastas (pelos snapshots salvos).\n  wdc mcp [--show]\n      Servidor MCP em stdio para agentes de IA. Comanda o app WinDevCleaner quando ele\n      está aberto (--show abre o app); senão roda sem janela.\n  wdc --version\n";
    CliUnknownCommand =>
        "Unknown command or option: {arg}. Run `wdc --help`.",
        "Comando ou opção desconhecida: {arg}. Rode `wdc --help`.";
    CliBadValue =>
        "Invalid value for {option}: {value}.",
        "Valor inválido para {option}: {value}.";
    CliScanStarting =>
        "Scanning {roots} in background mode...",
        "Analisando {roots} em modo de segundo plano...";
    CliScanDone =>
        "Scanned {files} files in {dirs} folders in {secs} s.",
        "Analisados {files} arquivos em {dirs} pastas em {secs} s.";
    CliSizeOnDisk =>
        "Size on disk: {size}",
        "Tamanho em disco: {size}";
    CliLargestFolders =>
        "Largest folders:",
        "Maiores pastas:";
    CliLargestFiles =>
        "Largest files:",
        "Maiores arquivos:";
    CliHardLinkNote =>
        "({links} names, counted once)",
        "({links} nomes, contado uma vez)";
    CliLinks =>
        "{n} links (junctions, symbolic links, mount points) recorded and not followed.",
        "{n} links (junções, links simbólicos, pontos de montagem) registrados e não seguidos.";
    CliHardLinkDupes =>
        "{n} extra names of hard-linked files were counted once, not twice.",
        "{n} nomes extras de arquivos com hard link foram contados uma vez, não duas.";
    CliPlaceholders =>
        "{n} cloud files not stored on this disk counted as 0 and never downloaded.",
        "{n} arquivos de nuvem que não estão neste disco contados como 0 e nunca baixados.";
    CliExcluded =>
        "{n} personal folders skipped (Documents, Desktop, Pictures, Videos, Music, Downloads, OneDrive).",
        "{n} pastas pessoais ignoradas (Documentos, Área de Trabalho, Imagens, Vídeos, Músicas, Downloads, OneDrive).";
    CliErrors =>
        "{n} items could not be read (access denied or gone); `--json` lists them.",
        "{n} itens não puderam ser lidos (acesso negado ou removidos); `--json` lista quais.";
    CliSnapshotSaved =>
        "Snapshot saved: {path} ({kept} kept, {size} in total).",
        "Snapshot salvo: {path} ({kept} guardados, {size} no total).";
    CliSnapshotFailed =>
        "Could not save the snapshot: {error}",
        "Não foi possível salvar o snapshot: {error}";
    CliScanFailed =>
        "Scan failed: {error}",
        "A análise falhou: {error}";
    CliCancelled =>
        "Scan cancelled.",
        "Análise cancelada.";

    // Desktop app.
    AppScan =>
        "Scan",
        "Analisar";
    AppScanning =>
        "Scanning in background mode... {files} files, {size}",
        "Analisando em segundo plano... {files} arquivos, {size}";
    AppScanSummary =>
        "{size} on disk in {files} files and {dirs} folders ({secs} s)",
        "{size} em disco em {files} arquivos e {dirs} pastas ({secs} s)";
    AppWhereIsMySpace =>
        "Where is my space?",
        "Onde está meu espaço?";
    AppNoScanYet =>
        "No scan yet. Press Scan to see where your space is. Nothing is changed on your disk.",
        "Nenhuma análise ainda. Clique em Analisar para ver onde está seu espaço. Nada é alterado no seu disco.";
    AppMoreItems =>
        "... and {n} more",
        "... e mais {n}";
    AppFiles =>
        "{n} files",
        "{n} arquivos";
    AppActivity =>
        "Agent activity",
        "Atividade do agente";
    AppNoActivity =>
        "No agent activity yet. Connect Claude Code, Codex or another MCP client to `wdc mcp`.",
        "Nenhuma atividade de agente ainda. Conecte o Claude Code, o Codex ou outro cliente MCP ao `wdc mcp`.";
    AppAgentConnected =>
        "Agent connected: {client}",
        "Agente conectado: {client}";
    AppNoAgent =>
        "No agent connected",
        "Nenhum agente conectado";
    AppAgentPausedBanner =>
        "Agent paused: its calls wait until you resume it.",
        "Agente pausado: as chamadas dele esperam até você retomar.";
    AppAgentsRefused =>
        "Agent disconnected. New agent connections are refused until you allow them.",
        "Agente desconectado. Novas conexões de agentes são recusadas até você permitir.";
    AppPauseAgent =>
        "Pause agent",
        "Pausar agente";
    AppResumeAgent =>
        "Resume agent",
        "Retomar agente";
    AppDisconnectAgent =>
        "Disconnect agent",
        "Desconectar agente";
    AppAllowAgents =>
        "Allow agent connections",
        "Permitir conexões de agentes";
    AppLinkUnavailable =>
        "Agent link unavailable: {error}",
        "Conexão com agentes indisponível: {error}";
    AppStatusRunning =>
        "running",
        "em andamento";
    AppStatusWaiting =>
        "waiting (agent paused)",
        "esperando (agente pausado)";
    AppStatusDone =>
        "done",
        "concluído";
    AppStatusFailed =>
        "failed",
        "falhou";
    AppStatusInfo =>
        "info",
        "info";
    AppActConnected =>
        "connected",
        "conectou";
    AppActDisconnected =>
        "disconnected: {reason}",
        "desconectou: {reason}";
    AppActScanning =>
        "scanning {roots}",
        "analisando {roots}";
    AppActTree =>
        "looking at {path}",
        "olhando {path}";
    AppActScanResult =>
        "{size} in {files} files",
        "{size} em {files} arquivos";
    AppActPaused =>
        "paused by you",
        "pausado por você";
    AppActResumed =>
        "resumed by you",
        "retomado por você";
    AppLanguage =>
        "Language",
        "Idioma";
    AppReadOnlyNote =>
        "Read-only: this version never changes your files.",
        "Somente leitura: esta versão nunca altera seus arquivos.";
    AppHumanScan =>
        "you",
        "você";

    // What grew (slice 2).
    CliDiffHeader =>
        "What changed between the scans of {from} and {to}: {delta} in total.",
        "O que mudou entre as análises de {from} e {to}: {delta} no total.";
    CliDiffNone =>
        "No folder changed by {min} or more.",
        "Nenhuma pasta mudou {min} ou mais.";
    CliDiffNeedTwo =>
        "Comparing needs two scans of the same folders. Run `wdc scan` again later.",
        "Para comparar são precisas duas análises das mesmas pastas. Rode `wdc scan` de novo mais tarde.";
    CliDiffFiles =>
        "Large files, new or grown:",
        "Arquivos grandes, novos ou que cresceram:";
    CliDiffMore =>
        "... and {n} more changes (see --limit).",
        "... e mais {n} mudanças (veja --limit).";
    DiffNew =>
        "new",
        "novo";
    DiffGrown =>
        "grew",
        "cresceu";
    DiffShrunk =>
        "shrank",
        "diminuiu";
    DiffRemoved =>
        "removed",
        "removido";

    // System items (slice 2).
    CliSystemItems =>
        "System items (report-only; Windows manages them):",
        "Itens do sistema (só relatório; o Windows os gerencia):";
    SysStatusAbsent =>
        "not present",
        "não existe";
    SysStatusNeedsAdmin =>
        "only an administrator can read it",
        "só um administrador pode ler";
    SysStatusError =>
        "could not be read",
        "não pôde ser lido";
    SysRecycleBin =>
        "Recycle Bin",
        "Lixeira";
    SysRecycleBinHow =>
        "Empty it from its desktop icon, or let Storage Sense do it (Settings > System > Storage).",
        "Esvazie pelo ícone na área de trabalho, ou deixe o Sensor de Armazenamento fazer isso (Configurações > Sistema > Armazenamento).";
    SysPageFile =>
        "Paging file",
        "Arquivo de paginação";
    SysPageFileHow =>
        "Managed by Windows: Settings > System > About > Advanced system settings > Performance > Advanced > Virtual memory.",
        "Gerenciado pelo Windows: Configurações > Sistema > Sobre > Configurações avançadas do sistema > Desempenho > Avançado > Memória virtual.";
    SysSwapFile =>
        "Swap file (Store apps)",
        "Arquivo de troca (apps da Store)";
    SysSwapFileHow =>
        "Managed by Windows together with the paging file.",
        "Gerenciado pelo Windows junto com o arquivo de paginação.";
    SysHibernation =>
        "Hibernation file",
        "Arquivo de hibernação";
    SysHibernationHow =>
        "`powercfg /h /type reduced` shrinks it; `powercfg /h off` removes it and turns off hibernation and Fast Startup. Both need an administrator terminal.",
        "`powercfg /h /type reduced` diminui o arquivo; `powercfg /h off` remove o arquivo e desliga a hibernação e a Inicialização Rápida. Os dois precisam de um terminal de administrador.";
    SysErrorReports =>
        "Windows error reports",
        "Relatórios de erros do Windows";
    SysErrorReportsHow =>
        "Settings > System > Storage > Temporary files > Windows error reports, or Disk Cleanup.",
        "Configurações > Sistema > Armazenamento > Arquivos temporários > Relatórios de erros do Windows, ou Limpeza de Disco.";
    SysCrashDumps =>
        "App crash dumps",
        "Despejos de falha de apps";
    SysCrashDumpsHow =>
        "Left by apps that crashed. Keep them while you debug a crash; otherwise Disk Cleanup removes them.",
        "Deixados por apps que travaram. Guarde enquanto investiga uma falha; senão a Limpeza de Disco os remove.";
    SysMemoryDumps =>
        "System memory dumps",
        "Despejos de memória do sistema";
    SysLiveKernelReports =>
        "Live kernel reports",
        "Relatórios do kernel em tempo real";
    SysMemoryDumpsHow =>
        "Disk Cleanup > Clean up system files > System error memory dump files (needs an administrator).",
        "Limpeza de Disco > Limpar arquivos do sistema > Arquivos de despejo de memória de erros do sistema (precisa de administrador).";
    SysSystemRestore =>
        "System Restore shadow storage",
        "Armazenamento de sombra da Restauração do Sistema";
    SysSystemRestoreHow =>
        "System Properties > System Protection > Configure. Its size needs an administrator (`vssadmin list shadowstorage`).",
        "Propriedades do Sistema > Proteção do Sistema > Configurar. O tamanho só é visível para administradores (`vssadmin list shadowstorage`).";

    // App, slice 2.
    AppTabGrowth =>
        "What grew?",
        "O que cresceu?";
    AppTabSystem =>
        "System items",
        "Itens do sistema";
    AppGrowthNone =>
        "No comparison yet: it needs two scans of the same folders. Scan now and again later.",
        "Nenhuma comparação ainda: são precisas duas análises das mesmas pastas. Analise agora e de novo mais tarde.";
    AppReadSystem =>
        "Read system items",
        "Ler itens do sistema";
    AppSystemNotRead =>
        "System items have not been read yet. They are always report-only: Windows manages them.",
        "Os itens do sistema ainda não foram lidos. São sempre só relatório: o Windows os gerencia.";
    AppActDiff =>
        "comparing the last two scans",
        "comparando as duas últimas análises";
    AppActSystem =>
        "reading system items",
        "lendo itens do sistema";

    // Explain (slice 3).
    FoundRunning =>
        "running: {names}",
        "em execução: {names}";
    FoundNotRunning =>
        "not running: {names}",
        "não está em execução: {names}";
    FoundNoProcessList =>
        "the list of running programs is not available",
        "a lista de programas em execução não está disponível";
    FoundAge =>
        "last changed {days} days ago (needs {need})",
        "alterado há {days} dias (precisa de {need})";
    FoundNoDate =>
        "its date cannot be read",
        "a data não pôde ser lida";
    FoundListed =>
        "listed in {file}",
        "listado em {file}";
    FoundNotListed =>
        "not listed in {file}",
        "não listado em {file}";
    FoundReferenced =>
        "referenced by {file}",
        "referenciado por {file}";
    FoundNotReferenced =>
        "not referenced by {file}",
        "não referenciado por {file}";
    FoundFileMissing =>
        "{file} not found",
        "{file} não encontrado";
    FoundBadFile =>
        "{file} could not be read",
        "{file} não pôde ser lido";
    FoundVarMissing =>
        "{file}: a variable is not set",
        "{file}: uma variável não está definida";
    FoundNameMatches =>
        "the name matches {pattern}",
        "o nome corresponde a {pattern}";
    FoundNameDiffers =>
        "the name does not match {pattern}",
        "o nome não corresponde a {pattern}";
    FoundDriverOlder =>
        "artifact {a} is older than the installed driver {i}",
        "o artefato {a} é anterior ao driver instalado {i}";
    FoundDriverEqual =>
        "artifact {a} is the installed driver {i}",
        "o artefato {a} é o driver instalado {i}";
    FoundDriverNewer =>
        "artifact {a} is newer than the installed driver {i}: it may be a pending update",
        "o artefato {a} é mais novo que o driver instalado {i}: pode ser uma atualização pendente";
    FoundNoDriver =>
        "no installed NVIDIA driver found",
        "nenhum driver NVIDIA instalado encontrado";
    FoundNoArtifactVersion =>
        "no driver version found in the artifact",
        "nenhuma versão de driver encontrada no artefato";
    FoundUnsupported =>
        "not supported: {what}",
        "não suportado: {what}";
    ForbidVolumeRoot =>
        "the volume root",
        "a raiz do volume";
    ForbidWindows =>
        "Windows",
        "o Windows";
    ForbidProgramFiles =>
        "Program Files: programs are removed only by their own uninstaller",
        "Arquivos de Programas: programas só saem pelo próprio desinstalador";
    ForbidUserProfile =>
        "the user profile itself",
        "a própria pasta do usuário";
    ForbidPersonalFolder =>
        "a personal folder (Documents, Desktop, Pictures, Videos, Music, Downloads)",
        "uma pasta pessoal (Documentos, Área de Trabalho, Imagens, Vídeos, Músicas, Downloads)";
    ForbidOneDrive =>
        "OneDrive",
        "OneDrive";
    ForbidSecrets =>
        "secrets (keys, credentials, SSH, GnuPG)",
        "segredos (chaves, credenciais, SSH, GnuPG)";
    ForbidVirtualDisk =>
        "a virtual disk or mailbox file (VHD, VHDX, PST, OST)",
        "um disco virtual ou arquivo de e-mail (VHD, VHDX, PST, OST)";
    ForbidMessenger =>
        "messenger data",
        "dados de mensageiros";
    ForbidBrowserProfile =>
        "browser profile data (only named cache folders may be touched)",
        "dados de perfil do navegador (só pastas de cache nomeadas podem ser tocadas)";
    ForbidQuarantine =>
        "the quarantine folder",
        "a pasta de quarentena";
    ForbidGit =>
        "a folder with a Git repository",
        "uma pasta com um repositório Git";
    WhyNoRule =>
        "no rule knows this path",
        "nenhuma regra conhece este caminho";
    WhyLocalRule =>
        "only built-in rules can make an item actionable; this is a local rule",
        "só regras embutidas podem tornar um item agível; esta é uma regra local";
    WhyReportOnly =>
        "the rule is report-only",
        "a regra é só de relatório";
    WhyAppManaged =>
        "the app manages it: use its own setting",
        "o app o gerencia: use a configuração dele";
    WhyNeverTouch =>
        "never touched",
        "nunca é tocado";
    WhyCondition =>
        "condition not met: {found}",
        "condição não atendida: {found}";
    WhyMissing =>
        "it does not exist",
        "não existe";
    WhyIsLink =>
        "it is a link: links are never items",
        "é um link: links nunca são itens";
    WhyAllowedRoot =>
        "an allowed root itself is never an item",
        "uma raiz permitida nunca é um item";
    WhyOutsideRoots =>
        "outside the allowed roots",
        "fora das raízes permitidas";
    WhyForbidden =>
        "forbidden: {why}",
        "proibido: {why}";
    ClassRegenerableCache =>
        "regenerable cache",
        "cache regenerável";
    ClassInstallLeftover =>
        "install leftover",
        "sobra de instalação";
    ClassAppManaged =>
        "managed by its app",
        "gerenciado pelo app";
    ClassNeverTouch =>
        "never touch",
        "nunca tocar";
    ActionQuarantine =>
        "can go to quarantine",
        "pode ir para a quarentena";
    ActionReportOnly =>
        "report only",
        "só relatório";
    ActionOpenAppSetting =>
        "use the app's own setting",
        "use a configuração do app";
    KindFolder =>
        "folder",
        "pasta";
    KindFile =>
        "file",
        "arquivo";
    KindMissing =>
        "does not exist",
        "não existe";
    KindLink =>
        "link",
        "link";
    ExplainItem =>
        "{item} ({kind})",
        "{item} ({kind})";
    ExplainInside =>
        "{path} is inside this item.",
        "{path} está dentro deste item.";
    ExplainSize =>
        "Size on disk: {size} in {files} files.",
        "Tamanho em disco: {size} em {files} arquivos.";
    ExplainFreed =>
        "Deleting it would free: {size}.",
        "Apagar liberaria: {size}.";
    ExplainLinked =>
        "{size} has {n} names; while one is outside this item, deleting it frees nothing of that file:",
        "{size} tem {n} nomes; enquanto um estiver fora deste item, apagar não libera nada desse arquivo:";
    ExplainOutside =>
        "(outside this item)",
        "(fora deste item)";
    ExplainRule =>
        "Rule: {title} ({class}; {action}).",
        "Regra: {title} ({class}; {action}).";
    ExplainLocalRuleNote =>
        "Local rule from {file}: always report-only.",
        "Regra local de {file}: sempre só relatório.";
    ExplainUnknown =>
        "No rule knows this path. It is reported, never actionable; an agent can investigate it and propose a rule with its evidence.",
        "Nenhuma regra conhece este caminho. Ele é relatado, nunca agido; um agente pode investigar e propor uma regra com a evidência.";
    ExplainEvidence =>
        "Why it is here: {text}",
        "Por que está aqui: {text}";
    ExplainSource =>
        "Source: {text}",
        "Fonte: {text}";
    ExplainCost =>
        "Cost of removing it: {text}",
        "Custo de remover: {text}";
    ExplainHow =>
        "How to manage it: {text}",
        "Como gerenciar: {text}";
    ExplainConditions =>
        "Conditions:",
        "Condições:";
    ExplainActionable =>
        "Actionable: yes. A plan (slice 4) could include it; nothing is changed now.",
        "Pode ser agido: sim. Um plano (slice 4) poderia incluí-lo; nada é alterado agora.";
    ExplainNotActionable =>
        "Actionable: no.",
        "Pode ser agido: não.";
    ExplainScope =>
        "Allowed root: {root}",
        "Raiz permitida: {root}";
    CliUnknownLarge =>
        "Large folders no rule knows (an agent can investigate them and propose a rule):",
        "Pastas grandes que nenhuma regra conhece (um agente pode investigar e propor uma regra):";
    DiffUnknownTag =>
        "no rule knows it",
        "nenhuma regra conhece";
    AppTabExplain =>
        "Explain",
        "Explicar";
    AppExplainNone =>
        "Nothing explained yet. Press “?” next to a folder, or let an agent explain one.",
        "Nada explicado ainda. Clique em “?” ao lado de uma pasta, ou deixe um agente explicar uma.";
    AppActExplain =>
        "explaining {path}",
        "explicando {path}";
}


#[cfg(test)]
mod tests {
    use super::*;

    fn placeholders(s: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = s;
        while let Some(start) = rest.find('{') {
            let after = &rest[start + 1..];
            match after.find('}') {
                Some(end) => {
                    out.push(after[..end].to_string());
                    rest = &after[end + 1..];
                }
                None => break,
            }
        }
        out.sort();
        out.dedup();
        out
    }

    #[test]
    fn every_message_has_both_languages_with_the_same_placeholders() {
        for &m in Msg::ALL {
            let en = m.text(Lang::En);
            let pt = m.text(Lang::PtBr);
            assert!(!en.trim().is_empty(), "{m:?} has no English text");
            assert!(!pt.trim().is_empty(), "{m:?} has no pt-BR text");
            assert_eq!(placeholders(en), placeholders(pt), "{m:?} placeholders differ");
        }
    }

    #[test]
    fn fills_placeholders() {
        let s = Lang::PtBr.fmt(Msg::AppAgentConnected, &[("client", "Claude Code")]);
        assert_eq!(s, "Agente conectado: Claude Code");
    }

    #[test]
    fn parses_lang() {
        assert_eq!(Lang::parse("pt-BR"), Some(Lang::PtBr));
        assert_eq!(Lang::parse("PT_br"), Some(Lang::PtBr));
        assert_eq!(Lang::parse("en"), Some(Lang::En));
        assert_eq!(Lang::parse("fr"), None);
    }
}
