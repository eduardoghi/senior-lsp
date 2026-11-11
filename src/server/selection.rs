//! Seleção expandida: cada pedido aumenta a seleção para o próximo nó da árvore em volta, do
//! nome para a expressão, o comando, o bloco, a função e a regra inteira.

use lsp_types::{Position, SelectionRange};

use super::navigation::Context;

impl Context<'_> {
    pub(crate) fn selection_ranges(&self, positions: &[Position]) -> Vec<SelectionRange> {
        positions.iter().map(|&p| self.selection_range(p)).collect()
    }

    fn selection_range(&self, position: Position) -> SelectionRange {
        let tree = &self.doc.tree;
        let offset = self.offset(position);
        // Do nó mais interno que contém a posição até a raiz.
        let mut chain = vec![tree.root()];
        loop {
            let last = *chain.last().unwrap_or(&tree.root());
            match tree
                .children(last)
                .find(|&c| tree.text_range(c).contains_inclusive(offset))
            {
                Some(child) => chain.push(child),
                None => break,
            }
        }
        let mut ranges: Vec<_> = chain.into_iter().map(|n| tree.text_range(n)).collect();
        // Nós com o mesmo trecho dariam passos que não mudam nada na tela.
        ranges.dedup();
        let mut result: Option<SelectionRange> = None;
        for range in ranges {
            result = Some(SelectionRange {
                range: self.range(range),
                parent: result.map(Box::new),
            });
        }
        result.unwrap_or(SelectionRange {
            range: lsp_types::Range::new(position, position),
            parent: None,
        })
    }
}
