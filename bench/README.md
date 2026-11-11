# Medição e comparação

Os dois scripts conversam com o executável pelo stdio, como o editor, e precisam da pasta de regras
exportadas pelo gerenciador de artefatos e do dicionário `.tbs`. Rode a partir da pasta de regras.

`bench.py` mede o tempo de abertura, de digitação e de cada pedido, a memória e a análise da pasta
inteira. Os tempos de pedidos e de digitação vêm de regras escolhidas por quem roda.

```sh
python3 -I bench/bench.py BINARIO sapiens.tbs . regras/REGRA.LSP/Rule.txt
```

`comparar.py` grava as respostas de um binário (diagnósticos da pasta inteira, de regras abertas
depois de várias edições, e as respostas dos pedidos de navegação) para comparar com `diff` ou
`cmp` as saídas de dois binários. Serve para provar que uma otimização não mudou o comportamento.

```sh
python3 -I bench/comparar.py antes sapiens.tbs . antes.out
python3 -I bench/comparar.py depois sapiens.tbs . depois.out
cmp antes.out depois.out
```
