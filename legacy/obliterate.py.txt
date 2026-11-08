import os
import subprocess

def menu():
    print("\n=== OBLITERATE PROGRAM ===")
    prog = input("Digite parte do nome do programa para buscar/destruir: ").strip()
    if not prog:
        print("Nada informado, retornando ao menu.")
        return

    # 1. Tenta desinstalar via Windows (WMIC)
    print(f"Tentando desinstalar '{prog}' via painel do Windows...")
    try:
        subprocess.run(f'wmic product where "Name like \'%{prog}%\'" call uninstall', shell=True)
    except Exception as e:
        print(f"Erro ao tentar desinstalar: {e}")

    # 2. Busca e apaga pastas (AppData, Program Files, ProgramData)
    user = os.getlogin()
    paths = [
        f"C:\\Users\\{user}\\AppData\\Local\\{prog}",
        f"C:\\Users\\{user}\\AppData\\Roaming\\{prog}",
        f"C:\\Program Files\\{prog}",
        f"C:\\Program Files (x86)\\{prog}",
        f"C:\\ProgramData\\{prog}"
    ]
    print("Procurando e deletando pastas residuais...")
    for p in paths:
        if os.path.exists(p):
            try:
                if os.path.isdir(p):
                    import shutil
                    shutil.rmtree(p)
                    print(f"[OK] Pasta removida: {p}")
                else:
                    os.remove(p)
                    print(f"[OK] Arquivo removido: {p}")
            except Exception as e:
                print(f"[ERRO] Não conseguiu remover {p}: {e}")
        else:
            print(f"[Skip] Não encontrado: {p}")

    print("Obliterate concluído. Confira relatórios/logs se necessário.\n")
