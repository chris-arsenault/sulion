import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ChangeEvent,
  type FormEvent,
  type Ref,
} from "react";

import { Icon } from "../icons";
import {
  renderPromptTemplate,
  type PromptTemplateValues,
  type PromptTemplateVariable,
} from "../lib/promptTemplate";
import { Overlay } from "./ui";
import "./PromptTemplateDialog.css";

interface PromptTemplateDialogProps {
  open: boolean;
  promptName: string;
  template: string;
  variables: PromptTemplateVariable[];
  onCancel: () => void;
  onSend: (text: string) => void;
}

export function PromptTemplateDialog({
  open,
  promptName,
  template,
  variables,
  onCancel,
  onSend,
}: PromptTemplateDialogProps) {
  const variableKey = variables.map((variable) => variable.name).join("\n");
  const emptyValues = useMemo<PromptTemplateValues>(
    () =>
      Object.fromEntries(
        variables.map((variable) => [variable.name, ""]),
      ) as PromptTemplateValues,
    [variables],
  );
  const [values, setValues] = useState<PromptTemplateValues>(emptyValues);
  const firstInputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    if (!open) return;
    setValues(emptyValues);
  }, [open, emptyValues]);

  useEffect(() => {
    if (!open) return;
    firstInputRef.current?.focus();
  }, [open, variableKey]);

  const rendered = useMemo(
    () => renderPromptTemplate(template, values),
    [template, values],
  );
  const canSend = variables.every(
    (variable) => (values[variable.name] ?? "").length > 0,
  );

  const submit = useCallback(
    (event: FormEvent) => {
      event.preventDefault();
      if (!canSend) return;
      onSend(rendered);
    },
    [canSend, onSend, rendered],
  );

  const updateValue = useCallback((name: string, value: string) => {
    setValues((current) => ({
      ...current,
      [name]: value,
    }));
  }, []);

  return (
    <Overlay
      open={open}
      onClose={onCancel}
      modal
      width="min(92vw, 620px)"
      maxHeight="78vh"
      title="Prompt Values"
      subtitle={promptName}
      leading={<Icon name="send" size={14} />}
    >
      <form className="prompt-template" onSubmit={submit}>
        <div className="prompt-template__fields">
          {variables.map((variable, index) => (
            <PromptTemplateField
              key={variable.name}
              variable={variable}
              value={values[variable.name] ?? ""}
              inputRef={index === 0 ? firstInputRef : undefined}
              onValueChange={updateValue}
            />
          ))}
        </div>

        <pre className="prompt-template__preview">{rendered}</pre>

        <div className="prompt-template__actions">
          <button type="button" onClick={onCancel}>
            Cancel
          </button>
          <button
            type="submit"
            className="prompt-template__send"
            disabled={!canSend}
          >
            Send
          </button>
        </div>
      </form>
    </Overlay>
  );
}

function PromptTemplateField({
  variable,
  value,
  inputRef,
  onValueChange,
}: {
  variable: PromptTemplateVariable;
  value: string;
  inputRef?: Ref<HTMLInputElement>;
  onValueChange: (name: string, value: string) => void;
}) {
  const onChange = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => {
      onValueChange(variable.name, event.target.value);
    },
    [onValueChange, variable.name],
  );

  return (
    <label className="prompt-template__field">
      <span className="prompt-template__label">
        ${variable.name}
        {variable.occurrences > 1 ? (
          <span className="prompt-template__occurrences">
            x{variable.occurrences}
          </span>
        ) : null}
      </span>
      <input
        ref={inputRef}
        type="text"
        value={value}
        onChange={onChange}
        aria-label={`Value for ${variable.name}`}
        autoComplete="off"
      />
    </label>
  );
}
