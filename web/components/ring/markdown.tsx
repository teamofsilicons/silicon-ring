import { Fragment, type ReactNode } from 'react';
import { marked, type Token, type Tokens } from 'marked';
import { CodeBlock } from '@teamofsilicons/silicon-ui/code-block';
import styles from './docs.module.css';

// Renders the repository's Markdown docs (udd/cli.md) at build time. The source is ours, so no HTML is passed through:
// raw HTML in the Markdown shows as text.

function slug(text: string) {
  return text.toLowerCase().replace(/`/g, '').replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');
}

function plain(tokens: Token[] | undefined): string {
  return (tokens ?? []).map(token => ('tokens' in token && token.tokens ? plain(token.tokens) : 'text' in token ? String(token.text) : '')).join('');
}

function inline(tokens: Token[] | undefined): ReactNode {
  return (tokens ?? []).map((token, index) => <Fragment key={index}>{inlineToken(token)}</Fragment>);
}

function inlineToken(token: Token): ReactNode {
  switch (token.type) {
    case 'text':
      return 'tokens' in token && token.tokens ? inline(token.tokens) : (token as Tokens.Text).text;
    case 'escape':
      return (token as Tokens.Escape).text;
    case 'strong':
      return <strong>{inline((token as Tokens.Strong).tokens)}</strong>;
    case 'em':
      return <em>{inline((token as Tokens.Em).tokens)}</em>;
    case 'del':
      return <del>{inline((token as Tokens.Del).tokens)}</del>;
    case 'codespan':
      return <code>{(token as Tokens.Codespan).text}</code>;
    case 'br':
      return <br />;
    case 'link': {
      const link = token as Tokens.Link;
      const external = /^https?:/.test(link.href);
      return <a href={link.href} className={styles.link} {...(external ? { target: '_blank', rel: 'noreferrer' } : {})}>{inline(link.tokens)}</a>;
    }
    case 'html':
      return (token as Tokens.HTML).text;
    default:
      return 'text' in token ? String(token.text) : null;
  }
}

function block(token: Token, index: number): ReactNode {
  switch (token.type) {
    case 'space':
    case 'hr':
      return token.type === 'hr' ? <hr key={index} className={styles.rule} /> : null;
    case 'heading': {
      const heading = token as Tokens.Heading;
      const id = slug(plain(heading.tokens));
      const Tag = (`h${Math.min(Math.max(heading.depth, 1), 4)}`) as 'h1' | 'h2' | 'h3' | 'h4';
      return <Tag key={index} id={id} className={styles[`h${heading.depth}`] ?? styles.h4}>{inline(heading.tokens)}</Tag>;
    }
    case 'paragraph':
      return <p key={index} className={styles.paragraph}>{inline((token as Tokens.Paragraph).tokens)}</p>;
    case 'text':
      return <p key={index} className={styles.paragraph}>{inlineToken(token)}</p>;
    case 'blockquote':
      return <blockquote key={index} className={styles.quote}>{(token as Tokens.Blockquote).tokens.map(block)}</blockquote>;
    case 'code': {
      const code = token as Tokens.Code;
      return <div key={index} className={styles.code}><CodeBlock code={code.text} language={code.lang || 'text'} /></div>;
    }
    case 'list': {
      const list = token as Tokens.List;
      const items = list.items.map((item, itemIndex) => (
        <li key={itemIndex}>
          {item.tokens.map((child, childIndex) => (child.type === 'text' ? <Fragment key={childIndex}>{inlineToken(child)}</Fragment> : block(child, childIndex)))}
        </li>
      ));
      return list.ordered
        ? <ol key={index} className={styles.list} start={typeof list.start === 'number' ? list.start : undefined}>{items}</ol>
        : <ul key={index} className={styles.list}>{items}</ul>;
    }
    case 'table': {
      const table = token as Tokens.Table;
      return (
        <div key={index} className={styles.tableWrap}>
          <table className={styles.table}>
            <thead>
              <tr>{table.header.map((cell, cellIndex) => <th key={cellIndex}>{inline(cell.tokens)}</th>)}</tr>
            </thead>
            <tbody>
              {table.rows.map((row, rowIndex) => (
                <tr key={rowIndex}>{row.map((cell, cellIndex) => <td key={cellIndex}>{inline(cell.tokens)}</td>)}</tr>
              ))}
            </tbody>
          </table>
        </div>
      );
    }
    case 'html':
      return <p key={index} className={styles.paragraph}>{(token as Tokens.HTML).text}</p>;
    default:
      return null;
  }
}

export function Markdown({ source }: { source: string }) {
  const tokens = marked.lexer(source, { gfm: true });
  return <div className={styles.markdown}>{tokens.map(block)}</div>;
}
