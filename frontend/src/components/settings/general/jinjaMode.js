// Adapted from MoviePilot-Frontend src/ace-config.ts (MIT).
import ace from 'ace-builds';

function registerJinja2Mode() {
  ace.define(
    'ace/mode/jinja2_highlight_rules',
    ['require', 'exports', 'module', 'ace/lib/oop', 'ace/mode/text_highlight_rules'],
    (require, exports) => {
      const oop = require('../lib/oop')
      const TextHighlightRules = require('./text_highlight_rules').TextHighlightRules

      const Jinja2HighlightRules = function () {
        const tags =
          'autoescape|block|call|do|elif|else|endautoescape|endblock|endcall|endfilter|endfor|endif|endmacro|endraw|endset|endtrans|endwith|extends|filter|for|from|if|import|include|macro|raw|set|trans|with'
        const filters =
          'abs|attr|batch|capitalize|center|count|d|default|dictsort|e|escape|filesizeformat|first|float|forceescape|format|groupby|indent|int|items|join|last|length|list|lower|map|max|min|pprint|random|reject|rejectattr|replace|reverse|round|safe|select|selectattr|slice|sort|string|striptags|sum|title|tojson|trim|truncate|unique|upper|urlencode|urlize|wordcount|wordwrap|xmlattr'
        const functions = 'cycler|dict|joiner|lipsum|namespace|range'
        const tests =
          'boolean|defined|divisibleby|eq|escaped|even|false|filter|float|ge|gt|in|integer|iterable|le|lower|lt|mapping|ne|none|number|odd|sameas|sequence|string|test|true|undefined|upper'
        const operators = 'and|in|is|not|or'
        const contextVariables =
          'title|en_title|original_title|season|season_fmt|year|title_year|type|category|vote_average|poster|backdrop|season_year|actors|overview|tmdbid|imdbid|doubanid|bangumiid|anilistid|episode_title|episode_date|original_name|name|en_name|episode|season_episode|part|customization|fps|resourceType|effect|edition|videoFormat|resource_term|releaseGroup|videoCodec|audioCodec|webSource|torrent_title|pubdate|freedate|seeders|volume_factor|hit_and_run|labels|description|site_name|size|transfer_type|file_count|total_size|err_msg|fileExt|__meta__|__mediainfo__|__torrentinfo__|__transferinfo__|__episodes_info__'

        const keywordMapper = this.createKeywordMapper(
          {
            'keyword.control.jinja2': tags,
            'keyword.operator.jinja2': operators,
            'support.function.jinja2': [filters, functions, tests].join('|'),
            'constant.language.jinja2': 'false|False|none|None|null|true|True',
          },
          'identifier',
        )

        const jinjaExpressionRules = [
          {
            token: 'string',
            regex: "'",
            push: 'jinja2-qstring',
          },
          {
            token: 'string',
            regex: '"',
            push: 'jinja2-qqstring',
          },
          {
            token: 'constant.numeric',
            regex: /[+-]?(?:0[xX][0-9a-fA-F]+|\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?\b/,
          },
          {
            token: ['keyword.operator.other.jinja2', 'text', 'support.function.jinja2'],
            regex: `(\\|)(\\s*)(${filters})\\b`,
          },
          {
            token: ['keyword.operator.jinja2', 'text', 'support.function.jinja2'],
            regex: `(\\bis\\b)(\\s*)(${tests})\\b`,
          },
          {
            token: ['support.function.jinja2', 'text', 'paren.lparen'],
            regex: `\\b(${functions})(\\s*)(\\()`,
          },
          {
            token: 'variable.language.jinja2',
            regex: `\\b(?:${contextVariables})\\b`,
          },
          {
            token: keywordMapper,
            regex: /[a-zA-Z_$][a-zA-Z0-9_$]*\b/,
          },
          {
            token: 'keyword.operator.assignment.jinja2',
            regex: /=|~/,
          },
          {
            token: 'keyword.operator.comparison.jinja2',
            regex: /==|!=|<=|>=|<|>/,
          },
          {
            token: 'keyword.operator.arithmetic.jinja2',
            regex: /\+|-|\/\/|\/|%|\*\*|\*/,
          },
          {
            token: 'keyword.operator.other.jinja2',
            regex: /\.{2}|\||:/,
          },
          {
            token: 'punctuation.operator.jinja2',
            regex: /[.,;?]/,
          },
          {
            token: 'paren.lparen',
            regex: /[\[({]/,
          },
          {
            token: 'paren.rparen',
            regex: /[\])}]/,
          },
          {
            token: 'text',
            regex: /\s+/,
          },
        ]

        this.$rules = {
          start: [
            {
              token: 'comment.block.jinja2',
              regex: /\{#-?/,
              push: 'jinja2-comment',
            },
            {
              token: 'constant.language.jinja2',
              regex: /\{\{-?/,
              push: 'jinja2-expression',
            },
            {
              token: 'keyword.control.jinja2',
              regex: /\{%-?/,
              push: 'jinja2-statement',
            },
          ],
          'jinja2-comment': [
            {
              token: 'comment.block.jinja2',
              regex: /-?#\}/,
              next: 'pop',
            },
            {
              defaultToken: 'comment.block.jinja2',
            },
          ],
          'jinja2-expression': [
            {
              token: 'constant.language.jinja2',
              regex: /-?\}\}/,
              next: 'pop',
            },
            ...jinjaExpressionRules,
          ],
          'jinja2-statement': [
            {
              token: 'keyword.control.jinja2',
              regex: /-?%\}/,
              next: 'pop',
            },
            ...jinjaExpressionRules,
          ],
          'jinja2-qqstring': [
            {
              token: 'constant.language.escape',
              regex: /\\[\\"ntr]/,
            },
            {
              token: 'string',
              regex: '"',
              next: 'pop',
            },
            {
              defaultToken: 'string',
            },
          ],
          'jinja2-qstring': [
            {
              token: 'constant.language.escape',
              regex: /\\[\\'ntr]/,
            },
            {
              token: 'string',
              regex: "'",
              next: 'pop',
            },
            {
              defaultToken: 'string',
            },
          ],
        }

        this.normalizeRules()
      }

      oop.inherits(Jinja2HighlightRules, TextHighlightRules)
      exports.Jinja2HighlightRules = Jinja2HighlightRules
    },
  )

  ace.define(
    'ace/mode/jinja2',
    ['require', 'exports', 'module', 'ace/lib/oop', 'ace/mode/text', 'ace/mode/jinja2_highlight_rules'],
    (require, exports) => {
      const oop = require('../lib/oop')
      const TextMode = require('./text').Mode
      const Jinja2HighlightRules = require('./jinja2_highlight_rules').Jinja2HighlightRules

      const Mode = function () {
        TextMode.call(this)
        this.HighlightRules = Jinja2HighlightRules
      }

      oop.inherits(Mode, TextMode)

      ;(function () {
        this.$id = 'ace/mode/jinja2'
        this.blockComment = { start: '{#', end: '#}' }
      }).call(Mode.prototype)

      exports.Mode = Mode
    },
  )

  ace.define('ace/snippets/jinja2', ['require', 'exports', 'module'], (_require, exports) => {
    exports.snippetText =
      'snippet if\n\t{% if ${1:condition} %}\n\t\t${0}\n\t{% endif %}\n' +
      'snippet for\n\t{% for ${1:item} in ${2:items} %}\n\t\t${0}\n\t{% endfor %}\n' +
      'snippet var\n\t{{ ${1:name} }}\n'
    exports.scope = 'jinja2'
  })

}

registerJinja2Mode();
