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
        "WinDevCleaner {version}: find where your disk space is, and clean it safely.\n\nUsage:\n  wdc scan [PATH ...] [--json] [--throttle MS] [--top N] [--depth N] [--jobs N] [--lang en|pt-BR]\n      Gentle scan (background mode) of the read scope, or of the given folders.\n      --top N shows the N largest folders and the N largest files (default 20 and 10) and keeps N files in\n      the JSON list `top_files` (default 50); the JSON `largest_dirs` is cut by --depth, not by --top.\n      --jobs N (1 to 4, default 1) walks with N threads, each in background mode: the same\n      report, faster on a cold disk, when you are waiting for it.\n      Saves a snapshot in the WinDevCleaner data folder; changes nothing else.\n  wdc explain PATH [--json]\n      Why a folder or file is here, the rule that knows it, and what deleting it would free.\n  wdc diff [--from SNAPSHOT] [--to SNAPSHOT] [--min-mb N] [--limit N] [--json]\n      What grew since the previous scan of the same folders (from the saved snapshots).\n  wdc plan [PATH ...] [--class CLASS] [--rule ID] [-o FILE] [--json]\n      Drafts a plan of what may move to quarantine now, each item explained. Changes nothing.\n  wdc plan show FILE [--json]\n  wdc apply --plan FILE [--purge-now] [--json]\n      Moves the plan's items to quarantine after re-checking each one. Needs proof that\n      you are here (Windows Hello, or a code shown in a desktop dialog).\n  wdc quarantine [--json]\n  wdc restore ID\n  wdc purge [--now] [ID ...]\n      Deletes quarantined items older than 7 days; --now also younger caches and install\n      leftovers. Restore and purge need the same proof.\n  wdc programs [--json]\n      Installed programs with the size each installer reports and its own uninstall command\n      (never run by this tool), then the folders left behind by programs uninstalled since the\n      last look. Read-only.\n  wdc mcp [--show]\n      MCP server on stdio for AI agents. Drives the WinDevCleaner app when it is open\n      (--show opens it); otherwise runs headless. Agents can never apply by themselves.\n  wdc --version\n",
        "WinDevCleaner {version}: descubra onde está o espaço do seu disco, e limpe com segurança.\n\nUso:\n  wdc scan [PASTA ...] [--json] [--throttle MS] [--top N] [--depth N] [--jobs N] [--lang en|pt-BR]\n      Análise suave (modo em segundo plano) do escopo de leitura, ou das pastas indicadas.\n      --top N mostra as N maiores pastas e os N maiores arquivos (padrão 20 e 10) e guarda N arquivos na\n      lista JSON `top_files` (padrão 50); `largest_dirs` no JSON é cortada por --depth, não por --top.\n      --jobs N (1 a 4, padrão 1) percorre com N threads, cada uma em modo em segundo plano: o\n      mesmo relatório, mais rápido num disco frio, quando você está esperando por ele.\n      Salva um snapshot na pasta de dados do WinDevCleaner; não altera mais nada.\n  wdc explain CAMINHO [--json]\n      Por que uma pasta ou arquivo está aqui, a regra que o conhece, e o que apagar liberaria.\n  wdc diff [--from SNAPSHOT] [--to SNAPSHOT] [--min-mb N] [--limit N] [--json]\n      O que cresceu desde a análise anterior das mesmas pastas (pelos snapshots salvos).\n  wdc plan [CAMINHO ...] [--class CLASSE] [--rule ID] [-o ARQUIVO] [--json]\n      Monta um plano do que pode ir para a quarentena agora, cada item explicado. Não altera nada.\n  wdc plan show ARQUIVO [--json]\n  wdc apply --plan ARQUIVO [--purge-now] [--json]\n      Move os itens do plano para a quarentena depois de checar cada um de novo. Precisa de\n      prova de que você está aqui (Windows Hello, ou um código numa janela da área de trabalho).\n  wdc quarantine [--json]\n  wdc restore ID\n  wdc purge [--now] [ID ...]\n      Apaga itens da quarentena com mais de 7 dias; --now também caches e sobras de instalação\n      mais novos. Restaurar e apagar precisam da mesma prova.\n  wdc programs [--json]\n      Programas instalados com o tamanho que cada instalador informa e o seu comando de\n      desinstalação (nunca executado por esta ferramenta), e as pastas deixadas por programas\n      desinstalados desde a última consulta. Só leitura.\n  wdc mcp [--show]\n      Servidor MCP em stdio para agentes de IA. Comanda o app WinDevCleaner quando ele\n      está aberto (--show abre o app); senão roda sem janela. Agentes nunca aplicam sozinhos.\n  wdc --version\n";
    CliUnknownCommand =>
        "Unknown command or option: {arg}. Run `wdc --help`.",
        "Comando ou opção desconhecida: {arg}. Rode `wdc --help`.";
    CliBadValue =>
        "Invalid value for {option}: {value}.",
        "Valor inválido para {option}: {value}.";
    CliJobsRange =>
        "--jobs takes a whole number from 1 to 4 (got: {value}).",
        "--jobs aceita um número inteiro de 1 a 4 (recebido: {value}).";
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
    AppScanFaster =>
        "Scan faster",
        "Analisar mais rápido";
    AppScanFasterHint =>
        "Uses 4 threads, still in background mode. Same result, quicker on a cold disk, but it competes more with your other work.",
        "Usa 4 threads, ainda em modo em segundo plano. Mesmo resultado, mais rápido num disco frio, mas disputa mais com o seu outro trabalho.";
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
        "Nothing on disk changes until you approve it here, and quarantined items can be restored.",
        "Nada no disco muda até você aprovar aqui, e itens na quarentena podem ser restaurados.";
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
    CliPlanLeftOutMore =>
        "... and {n} more left out (`--json` lists them all).",
        "... e mais {n} deixados de fora (`--json` lista todos).";
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
    SysStatusPartial =>
        "at least {size}; the rest needs an administrator",
        "pelo menos {size}; o resto precisa de um administrador";
    SysHibernationOff =>
        "hibernation is off",
        "a hibernação está desligada";
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
    WhyResolvesOutsideRoots =>
        "resolves through a link to {path}, outside the allowed roots",
        "passa por um link até {path}, fora das raízes permitidas";
    WhyForbidden =>
        "forbidden: {why}",
        "proibido: {why}";
    WhyNotFullyReadable =>
        "its size is incomplete: some of it could not be read",
        "o tamanho está incompleto: parte dele não pôde ser lida";
    WhyNothingToFree =>
        "deleting it would free nothing",
        "apagar não liberaria nada";
    WhyHeldOpen =>
        "held open by: {holders}. Close them and draft again.",
        "mantido aberto por: {holders}. Feche-os e gere o plano de novo.";
    WhyHeldOpenOne =>
        "held open by: {holders}. Close it and draft again.",
        "mantido aberto por: {holders}. Feche-o e gere o plano de novo.";
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
    ExplainSizeOne =>
        "Size on disk: {size} in 1 file.",
        "Tamanho em disco: {size} em 1 arquivo.";
    ExplainPartial =>
        "Partial: {n} items could not be read, so the real size may be larger.",
        "Parcial: {n} itens não puderam ser lidos, então o tamanho real pode ser maior.";
    ExplainPartialOne =>
        "Partial: 1 item could not be read, so the real size may be larger.",
        "Parcial: 1 item não pôde ser lido, então o tamanho real pode ser maior.";
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
    ExplainManualHeader =>
        "Manual steps (this tool never deletes it):",
        "Passos manuais (esta ferramenta nunca apaga isto):";
    ManualCloseApp =>
        "Close the NVIDIA app.",
        "Feche o app NVIDIA.";
    ManualOpenPowerShell =>
        "Open PowerShell as administrator.",
        "Abra o PowerShell como administrador.";
    ManualRunCommand =>
        "Run this command:",
        "Rode este comando:";
    ManualExplainAgain =>
        "Run `wdc explain` on the folder {parent} again to see what is left.",
        "Rode `wdc explain` de novo na pasta {parent} para ver o que sobrou.";
    ManualKeepNewer =>
        "Do not delete this artifact: its driver version {artifact} is newer than the installed driver {installed}, so it may be a pending update.",
        "Não apague este artefato: a versão de driver dele, {artifact}, é mais nova que o driver instalado, {installed}, então pode ser uma atualização pendente.";
    ManualKeepUnknown =>
        "Do not delete this artifact: the versions could not be compared ({why}).",
        "Não apague este artefato: não foi possível comparar as versões ({why}).";
    ExplainConditions =>
        "Conditions:",
        "Condições:";
    ExplainActionable =>
        "Actionable: yes. `wdc plan` can include it; nothing is changed now.",
        "Pode ser agido: sim. O `wdc plan` pode incluí-lo; nada é alterado agora.";
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
    CliPlanHeader =>
        "Plan {hash} ({at}): {n} items, {size} on disk, {freed} freed once purged.",
        "Plano {hash} ({at}): {n} itens, {size} em disco, {freed} liberados depois de apagados.";
    CliPlanFrees =>
        "frees {size}",
        "libera {size}";
    CliPlanEmpty =>
        "Nothing is eligible now.",
        "Nada pode ser movido agora.";
    CliPlanNotIncluded =>
        "Not included:",
        "Fora do plano:";
    CliPlanSaved =>
        "Plan saved: {path}",
        "Plano salvo: {path}";
    CliPlanNext =>
        "Review it with `wdc plan show {path}`, then apply it yourself with `wdc apply --plan {path}`.",
        "Revise com `wdc plan show {path}` e aplique você mesmo com `wdc apply --plan {path}`.";
    CliRefused =>
        "Refused: {why}",
        "Recusado: {why}";
    CliFreeSpace =>
        "Free space on {volume}: {before} before, {after} after ({measured}; expected {expected}).",
        "Espaço livre em {volume}: {before} antes, {after} depois ({measured}; esperado {expected}).";
    CliFreeSpaceMoved =>
        "Free space on {volume}: {before} before, {after} after ({measured}). The items are in quarantine, not deleted: the space returns when they are purged ({expected} expected).",
        "Espaço livre em {volume}: {before} antes, {after} depois ({measured}). Os itens estão na quarentena, não foram apagados: o espaço volta quando forem apagados ({expected} esperados).";
    CliApplyAsk =>
        "These items move to quarantine (nothing is deleted; `wdc restore` puts them back). This needs proof that you are here.",
        "Estes itens vão para a quarentena (nada é apagado; `wdc restore` devolve). Isto precisa de prova de que você está aqui.";
    CliApplyAskPurge =>
        "These items move to quarantine and caches and install leftovers are deleted right away. This needs proof that you are here.",
        "Estes itens vão para a quarentena e caches e sobras de instalação são apagados na hora. Isto precisa de prova de que você está aqui.";
    CliApplyWhat =>
        "Move {n} items ({size}) of WinDevCleaner plan {hash} to quarantine",
        "Mover {n} itens ({size}) do plano {hash} do WinDevCleaner para a quarentena";
    CliApplyMoved =>
        "Moved {n} items ({size}) to quarantine.",
        "{n} itens ({size}) movidos para a quarentena.";
    CliSkipped =>
        "skipped {path}: {why}",
        "ignorado {path}: {why}";
    CliApplyNext =>
        "Quarantine frees nothing yet: `wdc purge` deletes items after 7 days ({size} expected), `wdc restore ID` puts one back.",
        "A quarentena ainda não libera nada: `wdc purge` apaga os itens depois de 7 dias ({size} esperados), `wdc restore ID` devolve um.";
    CliPurged =>
        "Deleted {n} items ({size}) for good.",
        "{n} itens ({size}) apagados de vez.";
    CliQuarantineEmpty =>
        "Quarantine is empty.",
        "A quarentena está vazia.";
    CliQuarantineHeld =>
        "Quarantine holds {size} in {n} items (it frees nothing until purged).",
        "A quarentena guarda {size} em {n} itens (não libera nada até ser apagada).";
    CliQuarantineLine =>
        "{rule}, {days} days ago, at {stored}",
        "{rule}, há {days} dias, em {stored}";
    CliRestoreWhat =>
        "Put {stored} back at {original}",
        "Devolver {stored} para {original}";
    CliRestored =>
        "Restored {path}.",
        "{path} restaurado.";
    CliPurgeWhat =>
        "Delete quarantined items for good (up to {size})",
        "Apagar de vez itens da quarentena (até {size})";
    AppTabPlan =>
        "Plan",
        "Plano";
    AppTabQuarantine =>
        "Quarantine",
        "Quarentena";
    AppDraftPlan =>
        "Draft a plan",
        "Montar um plano";
    AppPlanNone =>
        "No plan yet. Draft one, or let an agent draft one.",
        "Nenhum plano ainda. Monte um, ou deixe um agente montar.";
    AppApprovalTitle =>
        "{client} asks you to approve this plan",
        "{client} pede sua aprovação para este plano";
    AppApprovalTitleYou =>
        "Approve this plan?",
        "Aprovar este plano?";
    AppApproveHello =>
        "Approve with Windows Hello",
        "Aprovar com o Windows Hello";
    AppApproveClick =>
        "Approve",
        "Aprovar";
    AppReject =>
        "Reject",
        "Recusar";
    AppApplyPlan =>
        "Move to quarantine...",
        "Mover para a quarentena...";
    AppApplyPurgeNow =>
        "Move and delete caches now...",
        "Mover e apagar caches agora...";
    AppFreeSpace =>
        "Free space on {volume}: {before} before, {after} after.",
        "Espaço livre em {volume}: {before} antes, {after} depois.";
    AppRestore =>
        "Restore",
        "Restaurar";
    AppPurgeOld =>
        "Delete items older than 7 days",
        "Apagar itens com mais de 7 dias";
    AppPurgeNow =>
        "Delete caches and leftovers now",
        "Apagar caches e sobras agora";
    AppActPlan =>
        "drafting a plan",
        "montando um plano";
    AppActQuarantine =>
        "reading the quarantine",
        "lendo a quarentena";
    AppActRequestApply =>
        "asking you to approve plan {hash}",
        "pedindo sua aprovação do plano {hash}";
    PlanUnknownFormat =>
        "This plan file has format {format}, which this version does not read. Draft the plan again.",
        "Este arquivo de plano tem formato {format}, que esta versão não lê. Gere o plano de novo.";
    PlanEdited =>
        "This plan was changed after it was written (its hash does not match). Draft it again with `wdc plan`.",
        "Este plano foi alterado depois de escrito (o hash não confere). Gere de novo com `wdc plan`.";
    PlanTooOld =>
        "This plan is {hours} hours old; plans older than 24 hours are refused. Draft it again with `wdc plan`.",
        "Este plano tem {hours} horas; planos com mais de 24 horas são recusados. Gere de novo com `wdc plan`.";
    PlanFromFuture =>
        "This plan is dated in the future. Check the clock and draft it again.",
        "Este plano tem data no futuro. Confira o relógio e gere de novo.";
    PresenceNoDesktop =>
        "No desktop to show the presence code on. Run this in your own Windows session.",
        "Não há área de trabalho para mostrar o código de presença. Rode isto na sua própria sessão do Windows.";
    PresenceCanceled =>
        "Windows Hello was cancelled. Nothing was changed.",
        "O Windows Hello foi cancelado. Nada foi alterado.";
    PresenceWrongCode =>
        "Wrong code. Nothing was changed.",
        "Código errado. Nada foi alterado.";
    PresenceTimedOut =>
        "No code was typed in time. Nothing was changed.",
        "Nenhum código foi digitado a tempo. Nada foi alterado.";
    PresenceNoInput =>
        "The console input ended without a code. Nothing was changed.",
        "A entrada do console terminou sem um código. Nada foi alterado.";
    PresenceHelloFailed =>
        "Windows Hello did not verify you ({detail}). Nothing was changed.",
        "O Windows Hello não confirmou você ({detail}). Nada foi alterado.";
    PresenceUnsupported =>
        "Presence cannot be proven on this system. Nothing was changed.",
        "Não é possível provar presença neste sistema. Nada foi alterado.";
    PresenceHelloPrompt =>
        "Confirm with Windows Hello (PIN, face or fingerprint)...",
        "Confirme com o Windows Hello (PIN, rosto ou digital)...";
    PresenceDialogText =>
        "WinDevCleaner needs to know a person is here.\n\nType this code in the console where you ran the command:\n\n{code}\n\nIf you did not run a WinDevCleaner command, close this and type nothing.",
        "O WinDevCleaner precisa saber que há uma pessoa aqui.\n\nDigite este código no console onde você rodou o comando:\n\n{code}\n\nSe você não rodou um comando do WinDevCleaner, feche isto e não digite nada.";
    PresenceTypeCode =>
        "A code is shown in a desktop dialog (never here). Type it and press Enter:",
        "Um código aparece numa janela da área de trabalho (nunca aqui). Digite e tecle Enter:";
    PurgeTooRecent =>
        "kept in quarantine for {days} more days (early purge is only for caches and install leftovers)",
        "fica na quarentena por mais {days} dias (apagar antes só vale para caches e sobras de instalação)";
    PurgeNotCache =>
        "early purge is only for regenerable caches and install leftovers; rule {rule} is neither",
        "apagar antes do prazo só vale para caches regeneráveis e sobras de instalação; a regra {rule} não é nenhum dos dois";
    PurgeNotExplained =>
        "its explanation was not shown when it was approved, so it waits the 7 days",
        "a explicação não foi mostrada na aprovação, então espera os 7 dias";
    PurgeOutside =>
        "the stored path {path} is not inside a quarantine folder; nothing deleted",
        "o caminho guardado {path} não está numa pasta de quarentena; nada apagado";
    PurgeChanged =>
        "what is in quarantine is not what was moved there; nothing done",
        "o que está na quarentena não é o que foi movido para lá; nada feito";
    PurgeFailed =>
        "could not delete: {error}",
        "não foi possível apagar: {error}";
    RestoreUnknown =>
        "No quarantined item has this id. `wdc quarantine` lists them.",
        "Nenhum item da quarentena tem este id. `wdc quarantine` lista os itens.";
    RestoreExists =>
        "{original} exists again (the app recreated it?). Both are kept: the quarantined copy stays at {stored}. Remove or rename one, then restore.",
        "{original} existe de novo (o app recriou?). Os dois foram mantidos: a cópia da quarentena continua em {stored}. Remova ou renomeie um, e restaure.";
    RestoreParentChanged =>
        "The folder it was in, {path}, is gone or is now a link. Nothing was moved.",
        "A pasta onde estava, {path}, sumiu ou agora é um link. Nada foi movido.";
    RestoreNotAllowed =>
        "Its original place is not one WinDevCleaner may act on now. Nothing was moved.",
        "O lugar original não é um onde o WinDevCleaner possa agir agora. Nada foi movido.";
    RestoreMissing =>
        "The quarantined copy is gone.",
        "A cópia da quarentena sumiu.";
    ApplyRenameFailed =>
        "could not move it ({error}); nothing forced",
        "não foi possível mover ({error}); nada foi forçado";
    ApplyOpenFailed =>
        "could not open it ({error}); nothing forced",
        "não foi possível abri-lo ({error}); nada foi forçado";
    ApplyHeldOpen =>
        "held open by {holders}; close it and run again",
        "mantido aberto por {holders}; feche-o e rode de novo";
    ApplyHeldOpenMany =>
        "held open by {holders}; close them and run again",
        "mantido aberto por {holders}; feche-os e rode de novo";
    ApplyNotAllowed =>
        "its real path {path} is not allowed: {why}",
        "o caminho real {path} não é permitido: {why}";
    ApplyChanged =>
        "it changed since the plan ({what}); draft the plan again",
        "mudou desde o plano ({what}); gere o plano de novo";
    ApplyNoMatch =>
        "no built-in rule matches {path} as an item now",
        "nenhuma regra embutida reconhece {path} como item agora";
    ApplyRuleChanged =>
        "another rule matches it now ({rule}); draft the plan again",
        "outra regra o reconhece agora ({rule}); gere o plano de novo";
    ApplyQuarantineUnsafe =>
        "the quarantine folder is not safe to use ({detail})",
        "a pasta de quarentena não é segura para usar ({detail})";

    // Programs (slice 7).
    ProgSizeNote =>
        "Sizes are what each installer reports (EstimatedSize in the registry), not a measured size.",
        "Os tamanhos são os que cada instalador informa (EstimatedSize no registro), não um tamanho medido.";
    ProgReadOnlyNote =>
        "Read-only: WinDevCleaner never runs an uninstall command and changes nothing. The commands are for the person to run.",
        "Só leitura: o WinDevCleaner nunca executa um comando de desinstalação e não altera nada. Os comandos são para a pessoa executar.";
    ProgWhyName =>
        "Named exactly like the program {program}, which is no longer installed.",
        "Tem exatamente o nome do programa {program}, que não está mais instalado.";
    ProgWhyPublisher =>
        "Named exactly like the publisher {publisher} of {program}, which is no longer installed, and no installed program has that publisher.",
        "Tem exatamente o nome do fabricante {publisher} de {program}, que não está mais instalado, e nenhum programa instalado é desse fabricante.";
    ProgLeftoverNote =>
        "Candidate only (install leftover): a person must confirm it before anything is done. Run {explain} to see what it is. The tool never removes it.",
        "Apenas candidata (sobra de instalação): uma pessoa precisa confirmar antes de qualquer ação. Rode {explain} para ver o que é. A ferramenta nunca a remove.";
    ProgScopeUser =>
        "this user",
        "este usuário";
    ProgScopeMachine =>
        "all users",
        "todos os usuários";
    CliProgramsHeader =>
        "Installed programs ({n}). The size is what each installer reports, not a measured size. WinDevCleaner never runs an uninstall command: copy one into your own terminal if you want it.",
        "Programas instalados ({n}). O tamanho é o que cada instalador informa, não um tamanho medido. O WinDevCleaner nunca executa um comando de desinstalação: copie um para o seu próprio terminal se quiser.";
    CliProgramsNone =>
        "No installed programs were found in the uninstall registry keys.",
        "Nenhum programa instalado foi encontrado nas chaves de desinstalação do registro.";
    CliProgramsNoSize =>
        "no size reported",
        "sem tamanho informado";
    CliProgramsNoCommand =>
        "no uninstall command recorded",
        "nenhum comando de desinstalação registrado";
    CliProgramsUninstall =>
        "uninstall: {command}",
        "desinstalar: {command}";
    CliProgramsQuiet =>
        "quiet uninstall: {command}",
        "desinstalar em silêncio: {command}";
    CliProgramsReadError =>
        "Could not read part of the registry: {error}. Folders left behind were not compared this time.",
        "Não foi possível ler parte do registro: {error}. As pastas deixadas para trás não foram comparadas desta vez.";
    CliProgramsLeftoversHeader =>
        "Folders left behind by programs uninstalled since the last look (candidates only; a person must confirm each):",
        "Pastas deixadas por programas desinstalados desde a última consulta (só candidatas; uma pessoa precisa confirmar cada uma):";
    CliProgramsLeftoversNone =>
        "No program was uninstalled since the last look that left a folder named exactly like it or its publisher.",
        "Nenhum programa desinstalado desde a última consulta deixou uma pasta com exatamente o seu nome ou o do fabricante.";
    CliProgramsFirstLook =>
        "This is the first look: the list was saved. Run it again after an uninstall to see what the program left behind.",
        "Esta é a primeira consulta: a lista foi salva. Rode de novo depois de uma desinstalação para ver o que o programa deixou.";
    ProgKeysUnreadable =>
        "{n} uninstall key(s) could not be read, so folders left behind were not checked this time.",
        "{n} chave(s) de desinstalação não puderam ser lidas, então as pastas deixadas para trás não foram verificadas desta vez.";
    AppProgramsQuietLabel =>
        "quiet uninstall:",
        "desinstalar em silêncio:";
    AppTabPrograms =>
        "Programs",
        "Programas";
    AppReadPrograms =>
        "Read installed programs",
        "Ler programas instalados";
    AppProgramsNotRead =>
        "Installed programs have not been read yet. This is read-only: the app never runs an uninstall command.",
        "Os programas instalados ainda não foram lidos. É só leitura: o app nunca executa um comando de desinstalação.";
    AppActPrograms =>
        "reading installed programs",
        "lendo programas instalados";
    AppActProgramsResult =>
        "{n} programs, {left} leftover candidates",
        "{n} programas, {left} pastas candidatas";
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
