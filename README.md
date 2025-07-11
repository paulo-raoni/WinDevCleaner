
# 🧹 WinDevCleaner

**Limpe, organize e destrua lixos digitais do seu Windows como um verdadeiro dev raiz. Modular, seguro, visual e com opções para quem gosta de viver perigosamente.**

---

## 🚀 O que é o WinDevCleaner?

O **WinDevCleaner** é o seu aliado para eliminar arquivos, caches, programas esquecidos e pastas órfãs do Windows, indo de uma limpeza leve (safe) até modos de destruição nuclear. Com menu interativo, logs detalhados, dry-run seguro e integração modular para plugins futuros.

- **Limpeza padrão ou extrema (Safe, Deep, Nuclear, Manual/Rankeada)**
- **Logs e relatórios em TXT, CSV e HTML**
- **Modo dry-run (simula), modo real-run (faz mesmo)**
- **Análise manual para grandes pastas (ex: Programs, pacotes Python)**
- **Fácil de usar: basta rodar no terminal, tudo guiado**

---

## 🗂️ Estrutura do Projeto

```plaintext
WinDevCleaner/
│
├── main.py                # Menu principal: limpeza, modos avançados, integração
├── cleaner.py             # Módulo com toda lógica de limpeza (safe/deep/nuclear)
├── programs_parser.py     # Análise/limpeza manual de pastas/programas gigantes
├── logs/                  # Logs em TXT por data
├── reports/               # Relatórios CSV e HTML por execução
├── README.md              # Você está aqui
└── (future) obliterate.py # Remoção e análise forense de programas (coming soon)
````

---

## 👨‍💻 Como rodar

> **Pré-requisitos:**
>
> * Python 3.8+
> * (Opcional, mas recomendado) Rodar como **Administrador** para deletar tudo

1. **Clone o repositório**

   ```sh
   git clone https://github.com/seuuser/windevcleaner.git
   cd windevcleaner
   ```

2. **(Opcional) Crie um ambiente virtual**

   ```sh
   python -m venv venv
   venv\Scripts\activate
   ```

3. **Instale as dependências**

   ```sh
   pip install colorama
   ```

4. **Rode o menu principal**

   ```sh
   python main.py
   ```

---

## 💡 Principais Funcionalidades

* **Modos de limpeza**

  * Safe: só cache/lixo seguro
  * Deep: vai em arquivos grandes e programas pouco críticos
  * Nuclear: pra quando só um apocalipse resolve
  * Manual/Rankeada: você escolhe o que quer apagar

* **Relatórios detalhados**

  * Espaço que será/foi liberado
  * Logs em TXT, CSV e HTML (salvos em `logs/` e `reports/`)

* **Dry-run e real-run**

  * Veja quanto será apagado ANTES de se arrepender

* **Extremamente modular**

  * Fácil criar novos modos, adicionar pastas, ou integrar com IA (!)

---

## ⚠️ AVISOS E ALERTAS

* **USE O MODO NUCLEAR/INSANITY POR SUA CONTA E RISCO!**
* Sempre cheque o relatório dry-run antes de executar real-run.
* Se deletar programas críticos, pode dar ruim. Leia, pense, depois execute.

---

## 📝 Roadmap (To-Do/Futuro)

* [x] Modularização por arquivos
* [x] Logs bonitos e relatórios automáticos
* [x] Manual de uso no terminal
* [ ] Modo **Obliterate**: desinstalar programas 100% (em breve)
* [ ] Integração IA para análise de riscos e sugestões (!)
* [ ] Instalação por pip/winget

---

## 🖼️ Exemplos Visuais

<details>
<summary>Clique para ver prints (logs/reports)...</summary>

![print-report-html](docs/imagens/exemplo-report.png)
![print-csv](docs/imagens/exemplo-csv.png)

</details>

---

## 🤝 Contribuindo

Contribuições são super bem-vindas!

* Abra uma Issue pra ideias ou bugs
* Faça um PR limpinho, explique bem
* Propague o caos de forma ética! 🧹💣

---

## 🦾 FAQ Rápido

* **Roda no Windows 10/11?**

  * Sim!
* **Pode deletar o próprio script?**

  * Só se escolher o modo *insanity* (não recomendado)
* **Dá pra plugar IA?**

  * Está nos planos! (Temos exemplos e hooks prontos)
* **Como saber o que vai apagar?**

  * Use sempre o dry-run e veja os relatórios!

---

## 👽 Licença

MIT – Limpe o que quiser, com responsabilidade!

---

### “Limpar nunca foi tão divertido. Menos pro seu disco, que chora de alegria!”

---
