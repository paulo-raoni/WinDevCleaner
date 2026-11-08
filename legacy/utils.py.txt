import datetime
import csv
from colorama import Fore, Style, init
init(autoreset=True)

def log(msg, color="white"):
    colors = {
        "red": Fore.RED,
        "green": Fore.GREEN,
        "yellow": Fore.YELLOW,
        "cyan": Fore.CYAN,
        "gray": Fore.LIGHTBLACK_EX,
        "blue": Fore.BLUE,
        "white": Fore.WHITE,
    }
    print(colors.get(color, Fore.WHITE) + msg + Style.RESET_ALL)

def save_report_csv(logs, filename="clean_report.csv"):
    with open(filename, "w", newline="", encoding="utf-8") as f:
        writer = csv.writer(f)
        writer.writerow(["Data", "Caminho", "Status"])
        for row in logs:
            writer.writerow(row)

def save_report_html(logs, filename="clean_report.html"):
    html = "<html><body><h2>Relatório de Limpeza</h2><table border=1><tr><th>Data</th><th>Caminho</th><th>Status</th></tr>"
    for log in logs:
        data, caminho, status = log
        color = "green" if "Removido" in status else "red" if "Falha" in status else "gray"
        html += f"<tr><td>{data}</td><td>{caminho}</td><td style='color:{color}'>{status}</td></tr>"
    html += "</table></body></html>"
    with open(filename, "w", encoding="utf-8") as f:
        f.write(html)
