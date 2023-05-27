## Overview

```
UP    M 1111 <hash>
DOWN  1111 <size> D<content hash>
UP    G 2222 0 0 <content hash>
DOWN  2222 <chunk num> <data>
DOWN  2222 <chunk num> <data>
DOWN  2222 <chunk num> <data>
DOWN  2222 <chunk num> <data>
UP    G 2222 0 90 <content hash>
DOWN  2222 <chunk num> <data>
DOWN  2222 <chunk num> <data>
```

## Commands

### GET

`G <id> <freq spec> <have> <hash>`
