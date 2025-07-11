import os
import shutil
import time
import csv
import subprocess
from colorama import init, Fore, Style

init(autoreset=True)

# ============================
# DEFINA OS CAMINHOS AQUI!
# ============================

targets_safe = [
    r"C:\Users\raoni\AppData\Local\Packages\PythonSoftwareFoundation.Python.3.10_qbz5n2kfra8p0\LocalCache\Local\pip\cache",
    r"C:\Users\raoni\AppData\Local\TechSmith\Camtasia Studio\CrashDumps",
    r"C:\Users\raoni\AppData\Local\uv\cache",
    r"C:\Users\raoni\AppData\Local\ms-playwright",
    r"C:\Users\raoni\AppData\Local\Microsoft\Edge\User Data\Default\Cache",
    r"C:\Users\raoni\AppData\Local\Microsoft\Edge\User Data\Profile 3\Cache",
    r"C:\Users\raoni\AppData\Local\CrashDumps",
]

targets_deep = targets_safe + [
    r"C:\Users\raoni\AppData\Local\Temp",
    r"C:\Users\raoni\AppData\Local\tiktok live studio-updater\pending",
    r"C:\Users\raoni\AppData\Local\tiktok live studio-updater\installer.exe",
    r"C:\Users\raoni\AppData\Local\Discord",
    r"C:\Users\raoni\AppData\Local\Programs\Opera",
    r"C:\Users\raoni\AppData\Local\slobs-client-updater",
    r"C:\Users\raoni\AppData\Local\Programs\Microsoft VS Code",
]

targets_nuclear = targets_deep + [
    r"C:\Users\raoni\AppData\Local\Packages\PythonSoftwareFoundation.Python.3.10_qbz5n2kfra8p0\LocalCache\local-packages",
    r"C:\Users\raoni\AppData\Local\Programs",
    r"C:\Users\raoni\AppData\Local\Microsoft\Edge\User Data",
    r"C:\Users\raoni\AppData\Local\ms-playwright",
    r"C:\Users\raoni\AppData\Local\docs",
    r"C:\Users\raoni\AppData\Local\Opera",
    # Acrescente o que mais quiser destruir! CUIDADO!!!
]

# ============================================
# FUNÇÕES PARA CALCULAR TAMANHO (bytes, MB, GB)
# ============================================

def get_size(path):
    total_size = 0
    if not os.path.exists(path):
        return 0
    if os.path.isfile(path):
        return os.path.getsize(path)
    for dirpath, dirnames, filenames in os.walk(path):
        for f in filenames:
            try:
                fp = os.path.join(dirpath, f)
                if os.path.exists(fp):
                    total_size += os.path.getsize(fp)
            except Exception:
                continue
    return total_size

def sizeof_fmt(num, suffix='B'):
    for unit in ['','K','M','G','T']:
        if abs(num) < 1024.0:
            return f"{num:3.1f} {unit}{suffix}"
        num /= 1024.0
    return f"{num:.1f} P{suffix}"

# ============================
# FUNÇÕES DE LIMPEZA
# ============================

def force_delete(path):
    try:
        if os.path.isdir(path):
            subprocess.run(["powershell", "-Command", f"Remove-Item -Path '{path}' -Recurse -Force"], shell=True)
        elif os.path.isfile(path):
            subprocess.run(["powershell", "-Command", f"Remove-Item -Path '{path}' -Force"], shell=True)
    except Exception as e:
        print(f"Falha ao forçar deleção de {path}: {e}")


def delete_path(path, dry_run=True, logs=[], sizes=[]):
    try:
        if not os.path.exists(path):
            logs.append((Fore.YELLOW + "Não encontrado", path, 0))
            return
        size = get_size(path)
        sizes.append(size)
        human = sizeof_fmt(size)

        if os.path.isfile(path):
            if dry_run:
                logs.append((Fore.CYAN + f"[Arquivo] {human}", path, size))
            else:
                os.remove(path)
                logs.append((Fore.RED + f"[Removido arquivo] {human}", path, size))
        else:
            if dry_run:
                logs.append((Fore.CYAN + f"[Pasta] {human}", path, size))
            else:
                shutil.rmtree(path)
                logs.append((Fore.RED + f"[Removido pasta] {human}", path, size))
    except Exception as e:
        msg = str(e)
        if "WinError 32" in msg or "em uso" in msg:
            logs.append((Fore.LIGHTRED_EX + "[Erro: Arquivo/Pasta em uso!]", path, 0))
            print(Fore.YELLOW + f"\nArquivo/pasta em uso: {path}")
            user_force = input("Tentar FORÇAR exclusão via PowerShell? (y/N): ").strip().lower()
            if user_force == "y":
                force_delete(path)
                logs.append((Fore.MAGENTA + "[Tentou FORCE DELETE]", path, 0))
        elif "WinError 5" in msg or "Permissão negada" in msg:
            logs.append((Fore.LIGHTRED_EX + "[Erro: Permissão negada! Rode como Administrador.]", path, 0))
            print(Fore.YELLOW + f"\nPermissão negada: {path}. Tente rodar como Admin.")
            user_force = input("Tentar FORÇAR exclusão via PowerShell? (y/N): ").strip().lower()
            if user_force == "y":
                force_delete(path)
                logs.append((Fore.MAGENTA + "[Tentou FORCE DELETE]", path, 0))
        else:
            logs.append((Fore.LIGHTRED_EX + f"[Erro: {e}]", path, 0))



def limpeza(paths, dry_run=True):
    logs = []
    sizes = []
    for path in paths:
        delete_path(path, dry_run, logs, sizes)
    return logs, sizes

def show_logs(logs, sizes):
    print(Fore.GREEN + "\n--- RELATÓRIO DA LIMPEZA ---\n")
    total = 0
    for (msg, p, sz) in logs:
        print(msg, Style.RESET_ALL, p)
        total += sz
    print(Fore.GREEN + f"\nTotal de caminhos analisados: {len(logs)}")
    print(Fore.MAGENTA + f"Total de espaço que seria {'liberado' if any(sz > 0 for sz in sizes) else 'analisado'}: {sizeof_fmt(total)}\n")

# ============================
# funções plug-and-play para salvar logs/relatórios em TXT, CSV e HTML
# ============================

def salvar_log_txt(logs, sizes, filename):
    with open(filename, "w", encoding="utf-8") as f:
        for (msg, p, sz) in logs:
            f.write(f"{msg} {p} ({sizeof_fmt(sz)})\n")
        f.write(f"\nTotal analisado: {len(logs)}\n")
        f.write(f"Total de espaço: {sizeof_fmt(sum(sizes))}\n")

def salvar_log_csv(logs, sizes, filename):
    with open(filename, "w", newline='', encoding="utf-8") as csvfile:
        writer = csv.writer(csvfile)
        writer.writerow(["Tipo", "Caminho", "Tamanho (bytes)", "Tamanho (humano)"])
        for (msg, p, sz) in logs:
            tipo = msg.split(']')[0][1:] if ']' in msg else msg
            writer.writerow([tipo, p, sz, sizeof_fmt(sz)])
        writer.writerow([])
        writer.writerow(["Total analisado", len(logs)])
        writer.writerow(["Total de espaço", sum(sizes), sizeof_fmt(sum(sizes))])

def salvar_log_html(logs, sizes, filename):
    html = [
        "<html><head><meta charset='utf-8'><title>Relatório de Limpeza</title></head><body>",
        "<h2>Relatório de Limpeza</h2>",
        "<table border='1'><tr><th>Tipo</th><th>Caminho</th><th>Tamanho</th></tr>"
    ]
    for (msg, p, sz) in logs:
        tipo = msg.split(']')[0][1:] if ']' in msg else msg
        html.append(f"<tr><td>{tipo}</td><td>{p}</td><td>{sizeof_fmt(sz)}</td></tr>")
    html.append("</table>")
    html.append(f"<p><b>Total analisado:</b> {len(logs)}</p>")
    html.append(f"<p><b>Total de espaço:</b> {sizeof_fmt(sum(sizes))}</p>")
    html.append("</body></html>")
    with open(filename, "w", encoding="utf-8") as f:
        f.write("\n".join(html))

# ============================
# MENU INTERATIVO EXPOSTO
# ============================

def menu():
    print(Fore.YELLOW + "\n### GERENCIADOR DE LIMPEZA PLAYWRIGHT-AUTOMATION ###\n")
    print(Fore.CYAN + "Níveis de limpeza disponíveis:")
    print("1 - SAFE    (padrão, seguro)")
    print("2 - DEEP    (intermediário, pode excluir programas ou caches grandes)")
    print("3 - NUCLEAR (EXTREMO, apaga quase tudo em AppData/Local!)\n")

    escolha = input(Fore.YELLOW + "Escolha o tipo de limpeza (1/2/3): ").strip()
    if escolha == "1":
        paths = targets_safe
        nivel = "SAFE"
    elif escolha == "2":
        paths = targets_deep
        nivel = "DEEP"
    elif escolha == "3":
        paths = targets_nuclear
        nivel = "NUCLEAR"
    else:
        print(Fore.LIGHTRED_EX + "Opção inválida. Usando SAFE.")
        paths = targets_safe
        nivel = "SAFE"

    print(Fore.YELLOW + f"\nVocê escolheu: {nivel}")

    modo = input(Fore.YELLOW + "Modo dry-run (simula, não apaga) ou real-run (apaga mesmo)? (d/r): ").strip().lower()
    dry = modo != "r"
    if dry:
        print(Fore.CYAN + "\n[DRY-RUN] Nada será apagado de verdade!\n")
    else:
        print(Fore.RED + "\n[REAL-RUN] Os arquivos/pastas serão APAGADOS MESMO!\n")
        conf = input(Fore.RED + "Tem certeza? Digite SIM para confirmar: ").strip().lower()
        if conf != "sim":
            print(Fore.YELLOW + "Cancelado pelo usuário.")
            return

    print(Fore.YELLOW + "\nIniciando a limpeza...\n")
    logs, sizes = limpeza(paths, dry)
    show_logs(logs, sizes)
    print(Fore.GREEN + "\nLimpeza finalizada! (revise o relatório acima)")
    os.makedirs("logs", exist_ok=True)
    os.makedirs("reports", exist_ok=True)
    timestamp = time.strftime("%Y%m%d-%H%M%S")
    salvar_log_txt(logs, sizes, f"logs/limpeza_{timestamp}.txt")
    salvar_log_csv(logs, sizes, f"reports/limpeza_{timestamp}.csv")
    salvar_log_html(logs, sizes, f"reports/limpeza_{timestamp}.html")
    print(Fore.CYAN + f"\nRelatórios salvos em logs/ e reports/")
    time.sleep(1)

# Se rodar direto: (não obrigatório, pois o main já chama o menu)
if __name__ == "__main__":
    menu()
