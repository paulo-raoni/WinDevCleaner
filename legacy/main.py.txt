from cleaner import menu as cleaner_menu
from obliterate import menu as obliterate_menu

def main_menu():
    while True:
        print("\n=== POWER CLEANER ===")
        print("1. Limpeza padrão (Cleaner)")
        print("2. Obliterate Program (Desinstalar + Apagar restos)")
        print("3. Sair")
        op = input("Escolha: ").strip()
        if op == "1":
            cleaner_menu()
        elif op == "2":
            obliterate_menu()
        elif op == "3":
            print("Saindo.")
            break
        else:
            print("Opção inválida.")

if __name__ == "__main__":
    main_menu()
