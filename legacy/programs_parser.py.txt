import os
import shutil
import sys
import time
import subprocess
from datetime import datetime
from colorama import init, Fore, Style

init(autoreset=True)

def get_dir_info(path):
    total_size = 0
    latest_mtime = 0
    latest_atime = 0
    exists = os.path.exists(path)
    if not exists:
        return None
    if os.path.isfile(path):
        total_size = os.path.getsize(path)
        latest_mtime = os.path.getmtime(path)
        latest_atime = os.path.getatime(path)
    else:
        for root, dirs, files in os.walk(path, topdown=True):
            for f in files:
                fp = os.path.join(root, f)
                try:
                    size = os.path.getsize(fp)
                    mtime = os.path.getmtime(fp)
                    atime = os.path.getatime(fp)
                    total_size += size
                    latest_mtime = max(latest_mtime, mtime)
                    latest_atime = max(latest_atime, atime)
                except Exception:
                    continue
    return {
        "path": path,
        "size": total_size,
        "mtime": latest_mtime,
        "atime": latest_atime,
        "isfile": os.path.isfile(path),
        "exists": exists
    }

def human_size(num, suffix="B"):
    for unit in ["", "K", "M", "G", "T"]:
        if abs(num) < 1024.0:
            return f"{num:.1f} {unit}{suffix}"
        num /= 1024.0
    return f"{num:.1f} P{suffix}"

def format_date(ts):
    if ts == 0:
        return "-"
    return datetime.fromtimestamp(ts).strftime("%Y-%m-%d %H:%M")

def scan_targets(targets):
    infos = []
    for t in targets:
        info = get_dir_info(t)
        if info and info["exists"]:
            infos.append(info)
    return sorted(infos, key=lambda x: (-x["size"], -x["mtime"]))

def print_ranked(infos):
    print(Fore.CYAN + "\n### Programas e Pastas Ranqueados por Espaço Ocupado ###\n")
    for i, info in enumerate(infos, 1):
        print(
            Fore.YELLOW
            + f"[{i}]"
            + Style.RESET_ALL
            + f" {human_size(info['size']):>10} | Últ. uso: {format_date(info['atime'])} | Últ. mod: {format_date(info['mtime'])} | {info['path']}"
        )
    print(Fore.GREEN + f"\nTotal encontrado: {len(infos)}\n")

def parse_selection(input_str, max_val):
    selected = set()
    for part in input_str.split(","):
        part = part.strip()
        if not part:
            continue
        if "-" in part:
            start, end = [int(x) for x in part.split("-")]
            selected.update(range(start, end + 1))
        else:
            selected.add(int(part))
    return sorted(i for i in selected if 1 <= i <= max_val)

def confirm(msg):
    return input(Fore.RED + msg + " (DIGITE SIM para confirmar): ").strip().lower() == "sim"

def delete_items(infos, to_delete, dry_run=True):
    total_freed = 0
    for i in to_delete:
        info = infos[i - 1]
        p = info["path"]
        if dry_run:
            print(Fore.CYAN + f"[DRY-RUN] Não apagou: {p} ({human_size(info['size'])})")
        else:
            try:
                if info["isfile"]:
                    os.remove(p)
                else:
                    shutil.rmtree(p)
                print(Fore.RED + f"[DELETADO] {p} ({human_size(info['size'])})")
                total_freed += info["size"]
            except Exception as e:
                print(Fore.LIGHTRED_EX + f"[Erro ao deletar {p}]: {e}")
    if not dry_run:
        print(Fore.GREEN + f"\nEspaço potencialmente liberado: {human_size(total_freed)}")

def main():
    print(Fore.YELLOW + "\n=== INSANITY MODE: ANÁLISE PROFUNDA DE PROGRAMAS ===")
    # Caminhos padrão (pode customizar!)
    targets = [
        r"C:\Users\raoni\AppData\Local\Programs",
        r"C:\Users\raoni\AppData\Local\Packages\PythonSoftwareFoundation.Python.3.10_qbz5n2kfra8p0\LocalCache\local-packages"
    ]
    print(Fore.LIGHTCYAN_EX + "\nEscaneando:\n" + "\n".join(targets))
    infos = scan_targets(targets)
    if not infos:
        print(Fore.LIGHTRED_EX + "Nenhum programa/pasta encontrado!")
        return

    print_ranked(infos)
    print(Fore.YELLOW + "Escolha quais deseja APAGAR (ex: 1,3,5-7), ou ENTER para não apagar nada:")
    sel = input(Fore.CYAN + "> ").strip()
    if not sel:
        print(Fore.YELLOW + "Nada será apagado. Saindo!")
        return
    to_del = parse_selection(sel, len(infos))
    if not to_del:
        print(Fore.YELLOW + "Seleção vazia ou inválida. Saindo!")
        return

    print(Fore.RED + "\nVocê selecionou para apagar:")
    for i in to_del:
        info = infos[i - 1]
        print(
            f"  [{i}] {human_size(info['size'])} | Últ. uso: {format_date(info['atime'])} | {info['path']}"
        )
    print(Fore.LIGHTRED_EX + "\nATENÇÃO: ISSO PODE TORNAR PROGRAMAS INUTILIZÁVEIS!")
    dry_run = True
    resp = input(Fore.YELLOW + "\nDry-run (simula, default) ou Real (apagar mesmo)? (d/r): ").strip().lower()
    if resp == "r":
        dry_run = False
        if not confirm("Tem certeza que deseja APAGAR PERMANENTEMENTE?"):
            print(Fore.YELLOW + "Cancelado.")
            return
    else:
        print(Fore.CYAN + "\nDRY-RUN ativado. Nada será apagado de verdade.")

    delete_items(infos, to_del, dry_run)
    print(Fore.GREEN + "\nFinalizado. Reveja o relatório acima.\n")

if __name__ == "__main__":
    main()
